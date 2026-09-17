//! Diagnostic-only, synchronous per-thread limits at the actual inference owners.
//! A refused call poisons the scope: even callers that swallow a reader error
//! cannot run another model. Production builds without `scan-profile` do nothing.

/// One actual network execution, including constructor contract probes.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Kind {
    Classifier,
    Ocr,
}

#[inline]
pub(crate) fn before(_kind: Kind) -> Result<(), String> {
    #[cfg(feature = "scan-profile")]
    return active::before(_kind);
    #[cfg(not(feature = "scan-profile"))]
    Ok(())
}

/// True while a call budget is being counted on this thread.
///
/// The budget is defined over one synchronous replay, so a model call made on
/// another thread would not be counted at all and the limit would quietly stop
/// applying. Work that would otherwise be spread out stays on this thread while
/// a scope is live.
pub(crate) fn scope_active() -> bool {
    #[cfg(feature = "scan-profile")]
    return active::scope_active();
    #[cfg(not(feature = "scan-profile"))]
    false
}

#[cfg(feature = "scan-profile")]
pub use active::Scope;

#[cfg(feature = "scan-profile")]
mod active {
    use super::Kind;
    use std::{cell::RefCell, marker::PhantomData, rc::Rc};

    struct Limit {
        max: [usize; 2],
        calls: [usize; 2],
        refused: Option<String>,
    }

    thread_local! {
        static ACTIVE: RefCell<Option<Limit>> = const { RefCell::new(None) };
    }

    pub(super) fn scope_active() -> bool {
        ACTIVE.with_borrow(Option::is_some)
    }

    /// Hard call budget for one synchronous replay. Not Send/Sync; no nesting.
    /// Keep alive while loading models too, to include constructor probes.
    pub struct Scope(PhantomData<Rc<()>>);

    impl Scope {
        /// Starts fresh counters. Refuses an overlapping scope without changing it.
        pub fn new(classifier: usize, ocr: usize) -> Result<Self, String> {
            ACTIVE.with_borrow_mut(|a| {
                if a.is_some() {
                    return Err("inference limit scope already active".into());
                }
                *a = Some(Limit {
                    max: [classifier, ocr],
                    calls: [0, 0],
                    refused: None,
                });
                Ok(Self(PhantomData))
            })
        }

        /// Actual attempted executions, including failed executions, not refusals.
        pub fn report(&self) -> serde_json::Value {
            ACTIVE.with_borrow(|a| {
                let a = a.as_ref().expect("live scope");
                serde_json::json!({"classifier_calls":a.calls[0],"ocr_calls":a.calls[1],
                    "max_classifier_calls":a.max[0],"max_ocr_calls":a.max[1],"refused":a.refused})
            })
        }

        /// Must be checked even when a scanner handled an optional reader error.
        pub fn check(&self) -> Result<(), String> {
            ACTIVE.with_borrow(|a| match &a.as_ref().expect("live scope").refused {
                Some(error) => Err(error.clone()),
                None => Ok(()),
            })
        }
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            ACTIVE.with_borrow_mut(|a| *a = None);
        }
    }

    pub(super) fn before(kind: Kind) -> Result<(), String> {
        ACTIVE.with_borrow_mut(|a| {
            let Some(a) = a else { return Ok(()) };
            if let Some(error) = &a.refused {
                return Err(error.clone());
            }
            let index = match kind {
                Kind::Classifier => 0,
                Kind::Ocr => 1,
            };
            if a.calls[index] == a.max[index] {
                let error = format!(
                    "inference limit reached before {kind:?} call; no further inference permitted"
                );
                a.refused = Some(error.clone());
                return Err(error);
            }
            a.calls[index] += 1;
            Ok(())
        })
    }
}

#[cfg(all(test, feature = "scan-profile"))]
mod tests {
    use super::*;

    #[test]
    fn refuses_before_execution_and_poison_stops_both_models() {
        let scope = Scope::new(1, 2).unwrap();
        let mut executions = 0;
        let mut run = |kind| -> Result<(), String> {
            before(kind)?;
            executions += 1;
            Ok(())
        };
        run(Kind::Classifier).unwrap();
        run(Kind::Ocr).unwrap();
        assert!(run(Kind::Classifier).is_err());
        // Optional repair can swallow the error, but not execute more work.
        assert!(run(Kind::Ocr).is_err());
        assert_eq!(executions, 2);
        assert!(scope.check().is_err());
        assert_eq!(scope.report()["classifier_calls"], 1);
        assert_eq!(scope.report()["ocr_calls"], 1);
    }

    #[test]
    fn scope_rejects_nesting_and_clears_on_drop_including_unwind() {
        let scope = Scope::new(0, 0).unwrap();
        assert!(Scope::new(99, 99).is_err());
        assert!(before(Kind::Ocr).is_err());
        drop(scope);
        assert!(
            std::panic::catch_unwind(|| {
                let _scope = Scope::new(1, 1).unwrap();
                panic!("test unwind");
            })
            .is_err()
        );
        let scope = Scope::new(1, 1).unwrap();
        before(Kind::Classifier).unwrap();
        before(Kind::Ocr).unwrap();
        assert!(scope.check().is_ok());
        drop(scope);
        before(Kind::Ocr).unwrap(); // ordinary unbounded caller unchanged
    }
}
