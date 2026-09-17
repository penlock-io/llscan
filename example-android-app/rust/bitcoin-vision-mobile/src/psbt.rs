//! A PSBT reviewed against a saved wallet's descriptor, and signed from
//! that review. Everything in the PSBT is the sender's claim: an origin
//! only nominates a wallet, and an input or output is the wallet's only
//! when its descriptor reproduces the key and the script.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use bdk_wallet::bitcoin::bip32::{ChildNumber, DerivationPath, Fingerprint, KeySource, Xpriv};
use bdk_wallet::bitcoin::hashes::Hash;
use bdk_wallet::bitcoin::key::{Keypair, TapTweak};
use bdk_wallet::bitcoin::psbt::{Psbt, PsbtSighashType};
use bdk_wallet::bitcoin::secp256k1::{Message, Secp256k1, Verification, XOnlyPublicKey};
use bdk_wallet::bitcoin::sighash::{Prevouts, SighashCache};
use bdk_wallet::bitcoin::{
    Address, Amount, Network, ScriptBuf, TapLeafHash, TapSighashType, TxOut, Witness, taproot,
};
use bdk_wallet::miniscript::{Descriptor, DescriptorPublicKey};
use rand::{RngCore, TryRngCore};

use crate::{VisionError, parse_words};

/// Above this the fee is refused outright, whatever the rate.
pub const FEE_CEILING: Amount = Amount::from_sat(1_000_000);

fn refuse(detail: impl Into<String>) -> VisionError {
    VisionError::Psbt {
        detail: detail.into(),
    }
}

/// A saved wallet as the review needs it: its identity and both chains.
struct Saved {
    fingerprint: Fingerprint,
    account: DerivationPath,
    descriptor: String,
    chains: [Descriptor<DescriptorPublicKey>; 2],
}

impl Saved {
    fn parse(descriptor: &str) -> Result<Saved, VisionError> {
        let parsed: Descriptor<DescriptorPublicKey> = descriptor
            .parse()
            .map_err(|e: bdk_wallet::miniscript::Error| refuse(format!("saved descriptor: {e}")))?;
        let key = match &parsed {
            Descriptor::Tr(tr) => tr.internal_key().clone(),
            _ => {
                return Err(refuse(
                    "saved descriptor is not a taproot key-path descriptor",
                ));
            }
        };
        let origin = match &key {
            DescriptorPublicKey::MultiXPub(x) => x.origin.clone(),
            DescriptorPublicKey::XPub(x) => x.origin.clone(),
            DescriptorPublicKey::Single(_) => None,
        };
        let (fingerprint, account) =
            origin.ok_or_else(|| refuse("saved descriptor has no key origin"))?;
        let chains: Vec<_> = parsed
            .into_single_descriptors()
            .map_err(|e| refuse(e.to_string()))?;
        let chains: [_; 2] = chains
            .try_into()
            .map_err(|_| refuse("saved descriptor does not have a receive and a change chain"))?;
        Ok(Saved {
            fingerprint,
            account,
            descriptor: descriptor.to_owned(),
            chains,
        })
    }

    /// The branch and index a claimed origin names under this wallet's
    /// account, if it names one at all.
    fn place(&self, origin: &KeySource) -> Option<(u32, u32)> {
        if origin.0 != self.fingerprint {
            return None;
        }
        let path: &[ChildNumber] = origin.1.as_ref();
        let account: &[ChildNumber] = self.account.as_ref();
        if path.len() != account.len() + 2 || &path[..account.len()] != account {
            return None;
        }
        match (path[account.len()], path[account.len() + 1]) {
            (ChildNumber::Normal { index: branch }, ChildNumber::Normal { index })
                if branch < 2 =>
            {
                Some((branch, index))
            }
            _ => None,
        }
    }

    /// The script and internal key this wallet has at `branch`/`index`.
    fn derive<C: Verification>(
        &self,
        secp: &Secp256k1<C>,
        branch: u32,
        index: u32,
    ) -> Result<(ScriptBuf, XOnlyPublicKey), VisionError> {
        let definite = self.chains[branch as usize]
            .at_derivation_index(index)
            .map_err(|e| refuse(e.to_string()))?;
        let script = definite.script_pubkey();
        let derived = definite
            .derived_descriptor(secp)
            .map_err(|e| refuse(e.to_string()))?;
        match derived {
            Descriptor::Tr(tr) => Ok((script, XOnlyPublicKey::from(tr.internal_key().inner))),
            _ => Err(refuse(
                "saved descriptor is not a taproot key-path descriptor",
            )),
        }
    }

    /// Whether this wallet's descriptor reproduces a claimed key-path
    /// key and the script beside it.
    fn reproduces<C: Verification>(
        &self,
        secp: &Secp256k1<C>,
        claimed: Option<(XOnlyPublicKey, &KeySource)>,
        script: &ScriptBuf,
    ) -> Result<Option<(u32, u32)>, VisionError> {
        let Some((key, source)) = claimed else {
            return Ok(None);
        };
        let Some((branch, index)) = self.place(source) else {
            return Ok(None);
        };
        let (ours, derived) = self.derive(secp, branch, index)?;
        Ok((derived == key && ours == *script).then_some((branch, index)))
    }
}

/// `total` plus `value`, refused once the sum leaves the money range:
/// each amount is checked alone, and the sums are held to it as well.
fn add(total: Amount, value: Amount, what: &str) -> Result<Amount, VisionError> {
    total
        .checked_add(value)
        .filter(|sum| *sum <= Amount::MAX_MONEY)
        .ok_or_else(|| refuse(format!("the {what} add up to more than there is")))
}

/// BIP371's claim about an input's or output's key-path key: the
/// origin recorded for its internal key.
fn claim(
    internal: Option<XOnlyPublicKey>,
    origins: &BTreeMap<XOnlyPublicKey, (Vec<TapLeafHash>, KeySource)>,
) -> Option<(XOnlyPublicKey, &KeySource)> {
    let key = internal?;
    let (_, source) = origins.get(&key)?;
    Some((key, source))
}

/// An output the transaction pays away from the wallet.
#[derive(Clone, Debug, PartialEq, uniffi::Record)]
pub struct OutputLine {
    /// The address, or the script in hex when it has none.
    pub address: String,
    /// Satoshis.
    pub amount: u64,
}

/// A PSBT checked against one saved wallet: the exact bytes and the
/// descriptor they were checked against, and what they do. Signing
/// consumes this and nothing else.
#[derive(Debug, uniffi::Object)]
pub struct Review {
    fingerprint: String,
    descriptor: String,
    inputs_total: u64,
    leaving: Vec<OutputLine>,
    change: u64,
    fee: u64,
    own_inputs: Vec<(usize, u32, u32)>,
    psbt: Psbt,
}

#[uniffi::export]
impl Review {
    /// The wallet the transaction spends from.
    pub fn fingerprint(&self) -> String {
        self.fingerprint.clone()
    }

    /// The descriptor the review held the transaction to.
    pub fn descriptor(&self) -> String {
        self.descriptor.clone()
    }

    /// Satoshis spent, all of them the wallet's.
    pub fn inputs_total(&self) -> u64 {
        self.inputs_total
    }

    /// What leaves the wallet, in transaction order.
    pub fn leaving(&self) -> Vec<OutputLine> {
        self.leaving.clone()
    }

    /// Satoshis coming back to the wallet.
    pub fn change(&self) -> u64 {
        self.change
    }

    /// Satoshis to the miners.
    pub fn fee(&self) -> u64 {
        self.fee
    }

    /// The exact bytes reviewed.
    pub fn psbt(&self) -> Vec<u8> {
        self.psbt.serialize()
    }
}

/// Reviews `psbt` against the saved `descriptors`. Every input must be
/// one wallet's — its descriptor deriving the input's key and the
/// spent script at the claimed path — unsigned, with its UTXO present
/// and the key-path default sighash; the outputs must be covered and
/// the fee under the ceiling. Anything else is refused with the reason.
#[uniffi::export]
pub fn review_psbt(descriptors: Vec<String>, psbt: Vec<u8>) -> Result<Arc<Review>, VisionError> {
    let psbt = Psbt::deserialize(&psbt).map_err(|e| refuse(format!("not a PSBT: {e}")))?;
    let saved = descriptors
        .iter()
        .map(|d| Saved::parse(d))
        .collect::<Result<Vec<_>, _>>()?;
    if psbt.inputs.is_empty() {
        return Err(refuse("the transaction has no inputs"));
    }
    if psbt.unsigned_tx.output.is_empty() {
        return Err(refuse("the transaction has no outputs"));
    }
    let mut spent = BTreeSet::new();
    for (i, txin) in psbt.unsigned_tx.input.iter().enumerate() {
        let n = i + 1;
        if !spent.insert(txin.previous_output) {
            return Err(refuse(format!(
                "input {n} spends the same coin as an earlier input"
            )));
        }
    }
    let secp = Secp256k1::verification_only();

    let mut nominated: Option<usize> = None;
    for (i, input) in psbt.inputs.iter().enumerate() {
        let (_, source) = claim(input.tap_internal_key, &input.tap_key_origins)
            .ok_or_else(|| refuse(format!("input {} has no taproot key origin", i + 1)))?;
        let which = saved
            .iter()
            .position(|w| w.fingerprint == source.0)
            .ok_or_else(|| {
                refuse(format!(
                    "input {} is not any saved wallet's: its key origin is {}",
                    i + 1,
                    source.0
                ))
            })?;
        match nominated {
            None => nominated = Some(which),
            Some(first) if first != which => {
                return Err(refuse(format!(
                    "the inputs are more than one wallet's: {} and {}",
                    saved[first].fingerprint, saved[which].fingerprint
                )));
            }
            Some(_) => {}
        }
    }
    let wallet = &saved[nominated.expect("at least one input")];

    let mut own_inputs = Vec::new();
    let mut inputs_total = Amount::ZERO;
    for (i, input) in psbt.inputs.iter().enumerate() {
        let n = i + 1;
        if input.final_script_sig.is_some()
            || input.final_script_witness.is_some()
            || !input.partial_sigs.is_empty()
            || !input.tap_script_sigs.is_empty()
            || input.tap_merkle_root.is_some()
        {
            return Err(refuse(format!(
                "input {n} already carries signature or script-path data"
            )));
        }
        let utxo = input
            .witness_utxo
            .as_ref()
            .ok_or_else(|| refuse(format!("input {n} has no UTXO")))?;
        if let Some(tx) = &input.non_witness_utxo {
            let spent = psbt.unsigned_tx.input[i].previous_output;
            if tx.compute_txid() != spent.txid || tx.output.get(spent.vout as usize) != Some(utxo) {
                return Err(refuse(format!("input {n}'s UTXO records disagree")));
            }
        }
        if utxo.value > Amount::MAX_MONEY {
            return Err(refuse(format!("input {n}'s amount is out of range")));
        }
        let claimed = claim(input.tap_internal_key, &input.tap_key_origins);
        let (branch, index) = wallet
            .reproduces(&secp, claimed, &utxo.script_pubkey)?
            .ok_or_else(|| {
                refuse(format!(
                    "input {n} is not this wallet's: the descriptor does not derive its key and script at the path it claims"
                ))
            })?;
        match input.sighash_type {
            None => {}
            Some(t) if t == PsbtSighashType::from(TapSighashType::Default) => {}
            Some(t) => {
                return Err(refuse(format!(
                    "input {n} asks for sighash {t}; only the key-path default is signed"
                )));
            }
        }
        inputs_total = add(inputs_total, utxo.value, "inputs")?;
        own_inputs.push((i, branch, index));
    }

    let mut leaving = Vec::new();
    let mut change = Amount::ZERO;
    let mut outputs_total = Amount::ZERO;
    for (j, out) in psbt.unsigned_tx.output.iter().enumerate() {
        if out.value > Amount::MAX_MONEY {
            return Err(refuse(format!("output {}'s amount is out of range", j + 1)));
        }
        outputs_total = add(outputs_total, out.value, "outputs")?;
        let meta = &psbt.outputs[j];
        let claimed = claim(meta.tap_internal_key, &meta.tap_key_origins);
        if wallet
            .reproduces(&secp, claimed, &out.script_pubkey)?
            .is_some()
        {
            change = add(change, out.value, "change outputs")?;
        } else {
            leaving.push(OutputLine {
                address: Address::from_script(&out.script_pubkey, Network::Bitcoin)
                    .map(|a| a.to_string())
                    .unwrap_or_else(|_| out.script_pubkey.to_hex_string()),
                amount: out.value.to_sat(),
            });
        }
    }
    let fee = inputs_total
        .checked_sub(outputs_total)
        .ok_or_else(|| refuse("the outputs exceed the inputs"))?;
    if fee > FEE_CEILING {
        return Err(refuse(format!(
            "the fee of {} sats is above the ceiling of {} sats",
            fee.to_sat(),
            FEE_CEILING.to_sat()
        )));
    }

    Ok(Arc::new(Review {
        fingerprint: wallet.fingerprint.to_string(),
        descriptor: wallet.descriptor.clone(),
        inputs_total: inputs_total.to_sat(),
        leaving,
        change: change.to_sat(),
        fee: fee.to_sat(),
        own_inputs,
        psbt,
    }))
}

/// Signs the reviewed transaction with `words`, which must be the key
/// of the review's descriptor: a key-path Schnorr signature with the
/// default sighash on every input the review found to be the wallet's,
/// each verified against the script it spends before the input is
/// finalized. The finalized PSBT comes back only once it extracts to
/// the reviewed transaction with nothing but those witnesses added.
#[uniffi::export]
pub fn sign_review(review: Arc<Review>, words: Vec<String>) -> Result<Vec<u8>, VisionError> {
    if crate::descriptor(words.clone())?.descriptor != review.descriptor {
        return Err(refuse("that phrase is not this wallet's key"));
    }
    let words = parse_words(&words)?;
    let seed = penlock::oracle::seed(&words, "")
        .ok_or_else(|| refuse("the phrase does not pass the BIP39 checksum"))?;
    let secp = Secp256k1::new();
    let master = Xpriv::new_master(Network::Bitcoin, &seed).map_err(|e| refuse(e.to_string()))?;
    let wallet = Saved::parse(&review.descriptor)?;

    let mut psbt = review.psbt.clone();
    let prevouts: Vec<TxOut> = psbt
        .inputs
        .iter()
        .map(|input| input.witness_utxo.clone())
        .collect::<Option<_>>()
        .ok_or_else(|| refuse("the review lost an input's UTXO"))?;
    let reviewed = psbt.unsigned_tx.clone();
    let mut cache = SighashCache::new(&reviewed);
    let mut rng = rand::rngs::OsRng.unwrap_err();
    for &(i, branch, index) in &review.own_inputs {
        let n = i + 1;
        let path = wallet
            .account
            .child(ChildNumber::from_normal_idx(branch).map_err(|e| refuse(e.to_string()))?)
            .child(ChildNumber::from_normal_idx(index).map_err(|e| refuse(e.to_string()))?);
        let child = master
            .derive_priv(&secp, &path)
            .map_err(|e| refuse(e.to_string()))?;
        let keypair = Keypair::from_secret_key(&secp, &child.private_key);
        let claimed = psbt.inputs[i]
            .tap_internal_key
            .ok_or_else(|| refuse(format!("input {n} lost its key")))?;
        if keypair.x_only_public_key().0 != claimed {
            return Err(refuse(format!("input {n}'s key is not the loaded key's")));
        }
        let tweaked = keypair.tap_tweak(&secp, None);
        let sighash = cache
            .taproot_key_spend_signature_hash(i, &Prevouts::All(&prevouts), TapSighashType::Default)
            .map_err(|e| refuse(e.to_string()))?;
        let message = Message::from_digest(sighash.to_byte_array());
        let mut aux = [0u8; 32];
        rng.fill_bytes(&mut aux);
        let signature = secp.sign_schnorr_with_aux_rand(&message, &tweaked.to_keypair(), &aux);
        // Verified against the key the spent script commits to, not the
        // key just used: the script is what the chain will check.
        let spent = prevouts[i].script_pubkey.as_bytes();
        let spent_key = spent
            .get(2..34)
            .and_then(|k| XOnlyPublicKey::from_slice(k).ok())
            .ok_or_else(|| refuse(format!("input {n} does not spend a taproot output")))?;
        secp.verify_schnorr(&signature, &message, &spent_key)
            .map_err(|_| refuse(format!("input {n}'s signature does not verify")))?;
        let signature = taproot::Signature {
            signature,
            sighash_type: TapSighashType::Default,
        };
        let input = &mut psbt.inputs[i];
        input.final_script_sig = None;
        input.final_script_witness = Some(Witness::p2tr_key_spend(&signature));
        input.tap_key_sig = None;
        input.tap_key_origins.clear();
        input.tap_internal_key = None;
    }
    for output in &mut psbt.outputs {
        output.tap_key_origins.clear();
        output.tap_internal_key = None;
    }

    // What leaves must be the reviewed transaction plus the witnesses
    // just made, and nothing else.
    let signed = psbt
        .clone()
        .extract_tx()
        .map_err(|e| refuse(format!("the signed transaction does not extract: {e}")))?;
    if signed.compute_txid() != reviewed.compute_txid() {
        return Err(refuse("the signed transaction is not the reviewed one"));
    }
    for (i, txin) in signed.input.iter().enumerate() {
        if !txin.script_sig.is_empty() || txin.witness.len() != 1 {
            return Err(refuse(format!(
                "input {} is not a key-path spend after signing",
                i + 1
            )));
        }
    }
    Ok(psbt.serialize())
}
