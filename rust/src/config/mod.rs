/// A config struct whose every field has a default, so a job file names only what it changes; unknown keys
/// are an error, so typos are caught.
macro_rules! config_struct {
    ($(#[$smeta:meta])* $name:ident { $($(#[$fmeta:meta])* $field:ident : $ty:ty = $default:expr),* $(,)? }) => {
        $(#[$smeta])*
        #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
        #[serde(default, deny_unknown_fields)]
        pub struct $name {
            $($(#[$fmeta])* pub $field: $ty,)*
        }

        impl Default for $name {
            fn default() -> Self {
                Self { $($field: $default,)* }
            }
        }
    };
}

pub mod run_configuration;
pub mod tuning;
