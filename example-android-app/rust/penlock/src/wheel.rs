//! The paper wheel, as a function.
//!
//! The wheel's top disk carries a pointer and 29 numbered windows; the bottom
//! disk carries the character ring the pointer reads and, under each window,
//! a ring of characters. With the pointer on `s`, window `w` reveals
//! `s·(w+15)`, except that when `s` is `=` it reveals `w` itself. This
//! reproduces the `translate` table of the upstream SDK.

use crate::field::Symbol;

/// One of the 29 numbered windows on the wheel's top disk.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Window(u8);

impl Window {
    /// Window 1, which follows window 29 in numbering order.
    pub const FIRST: Window = Window(1);
    /// Window 29 multiplies by 15 = 2⁻¹, which is how shares 1 and 3 recover.
    pub const LAST: Window = Window(29);

    /// The window printed with `number`, for `1..=29`; `None` otherwise.
    pub fn new(number: u8) -> Option<Window> {
        (1..=29).contains(&number).then_some(Window(number))
    }

    /// The number printed next to the window, in `1..=29`.
    pub fn number(self) -> u8 {
        self.0
    }

    /// The next window in numbering order; window 29 wraps to window 1.
    pub fn next(self) -> Window {
        Window(self.0 % 29 + 1)
    }

    /// Windows 1 through 29 in order.
    pub fn all() -> impl Iterator<Item = Window> {
        (1..=29).map(Window)
    }

    /// The character shown in this window when the pointer is on `pointer`.
    pub fn show(self, pointer: Symbol) -> Symbol {
        if pointer.is_zero() {
            Symbol::from_residue(u32::from(self.0))
        } else {
            pointer * Symbol::from_residue(u32::from(self.0) + 15)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_wrap() {
        assert_eq!(Window::LAST.next(), Window::FIRST);
        assert_eq!(Window::new(0), None);
        assert_eq!(Window::new(30), None);
        assert_eq!(Window::all().count(), 29);
    }

    #[test]
    fn window_29_halves() {
        for s in Symbol::all() {
            let doubled = s + s;
            assert_eq!(Window::LAST.show(doubled), s);
        }
    }
}
