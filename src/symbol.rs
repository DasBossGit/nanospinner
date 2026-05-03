use ::std::borrow::Cow;

pub trait Symbol {
    fn symbol(&self) -> &str;
    #[inline(always)]
    fn color(&self) -> Option<String> {
        None
    }
}

macro_rules! impl_symbol_for_char {
    ($($t:ty),*) => {
        $(
            impl Symbol for $t {
                fn symbol(&self) -> &str {
                    self.as_ref()
                }
            }
        )*
    };
}

impl_symbol_for_char!(&'static str, String, Cow<'static, str>, Box<str>);

#[derive(Debug, Clone, Copy)]
#[repr(u32)]
pub enum Symbols {
    Success = '✔' as u32,
    Warn = '⚠' as u32,
    Fail = '✖' as u32,
    Info = 'ℹ' as u32,
}

impl Symbol for Symbols {
    fn symbol(&self) -> &str {
        match self {
            Symbols::Success => "✔",
            Symbols::Warn => "⚠",
            Symbols::Fail => "✖",
            Symbols::Info => "ℹ",
        }
    }
}

impl AsRef<str> for Symbols {
    fn as_ref(&self) -> &str {
        self.symbol()
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum AsciiColor {
    // Standard (30–37)
    Black = 30,
    Red = 31,
    Green = 32,
    Yellow = 33,
    Blue = 34,
    Magenta = 35,
    Cyan = 36,
    White = 37,

    // Bright (90–97)
    BrightBlack = 90,
    BrightRed = 91,
    BrightGreen = 92,
    BrightYellow = 93,
    BrightBlue = 94,
    BrightMagenta = 95,
    BrightCyan = 96,
    BrightWhite = 97,
}

impl AsciiColor {
    pub fn to_ansi_code(&self) -> String {
        format!("\x1b[{}m", *self as u8)
    }
}

impl<T: AsRef<str>> Symbol for (T, AsciiColor) {
    fn symbol(&self) -> &str {
        self.0.as_ref()
    }

    fn color(&self) -> Option<String> {
        Some(self.1.to_ansi_code())
    }
}

impl Symbol for &dyn Symbol {
    fn symbol(&self) -> &str {
        Symbol::symbol(*self)
    }

    fn color(&self) -> Option<String> {
        Symbol::color(*self)
    }
}

impl Symbol for Box<dyn Symbol> {
    fn symbol(&self) -> &str {
        Symbol::symbol(self.as_ref())
    }

    fn color(&self) -> Option<String> {
        Symbol::color(self.as_ref())
    }
}
impl Symbol for Box<dyn Symbol + Send> {
    fn symbol(&self) -> &str {
        Symbol::symbol(self.as_ref())
    }

    fn color(&self) -> Option<String> {
        Symbol::color(self.as_ref())
    }
}

impl<S: Symbol> Symbol for &S {
    fn symbol(&self) -> &str {
        Symbol::symbol(*self)
    }

    fn color(&self) -> Option<String> {
        Symbol::color(*self)
    }
}
