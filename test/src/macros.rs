// A simple macro to create a Vec from a list of expressions with trailing comma support.
#[macro_export]
macro_rules! make_vec {
    ($($elem:expr),* $(,)?) => {{
        let mut v = ::std::vec::Vec::new();
        $(v.push($elem);)*
        v
    }};
}

// Macro that defines a newtype wrapper with common conversions and Debug/Clone/Copy/PartialEq.
#[macro_export]
macro_rules! define_newtype {
    ($name:ident, $inner:ty) => {
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        pub struct $name(pub $inner);

        impl From<$inner> for $name {
            fn from(value: $inner) -> Self {
                Self(value)
            }
        }

        impl From<$name> for $inner {
            fn from(value: $name) -> $inner {
                value.0
            }
        }
    };
}
