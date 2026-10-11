//! Internal macros.

/// Generate `with_*` builder methods that set one public field each.
///
/// ```ignore
/// with_setters! {
///     WorkerConfig {
///         with_pool_size(pool_size): usize,
///         with_batch_size(batch_size): Some(usize),   // sets `Some(value)`
///         with_user_agent(user_agent): Into(String),  // takes `impl Into<String>`
///     }
/// }
/// ```
///
/// Configuration structs are `#[non_exhaustive]`, so code outside the crate builds them
/// from `Default` (or a constructor) and these setters instead of struct literals.
macro_rules! with_setters {
    ($ty:ty { $($items:tt)* }) => {
        impl $ty {
            with_setters!(@items $($items)*);
        }
    };
    (@items) => {};
    (@items $(#[$attr:meta])* $name:ident($field:ident): Some($fty:ty) $(, $($rest:tt)*)?) => {
        #[doc = concat!(
            "Sets [`", stringify!($field), "`](Self::", stringify!($field), ") to `Some(value)`."
        )]
        $(#[$attr])*
        #[must_use]
        pub fn $name(mut self, value: $fty) -> Self {
            self.$field = Some(value);
            self
        }
        with_setters!(@items $($($rest)*)?);
    };
    (@items $(#[$attr:meta])* $name:ident($field:ident): Into($fty:ty) $(, $($rest:tt)*)?) => {
        #[doc = concat!("Sets [`", stringify!($field), "`](Self::", stringify!($field), ").")]
        $(#[$attr])*
        #[must_use]
        pub fn $name(mut self, value: impl Into<$fty>) -> Self {
            self.$field = value.into();
            self
        }
        with_setters!(@items $($($rest)*)?);
    };
    (@items $(#[$attr:meta])* $name:ident($field:ident): $fty:ty $(, $($rest:tt)*)?) => {
        #[doc = concat!("Sets [`", stringify!($field), "`](Self::", stringify!($field), ").")]
        $(#[$attr])*
        #[must_use]
        pub fn $name(mut self, value: $fty) -> Self {
            self.$field = value;
            self
        }
        with_setters!(@items $($($rest)*)?);
    };
}
