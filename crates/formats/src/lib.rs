pub mod rpgmaker_mv;
pub mod rpgmaker_vxa;
pub mod renpy;
pub mod wolf_rpg;

use locust_core::extraction::FormatRegistry;

pub fn default_registry() -> FormatRegistry {
    let mut r = FormatRegistry::new();
    r.register(Box::new(rpgmaker_mv::RpgMakerMvPlugin::new()));
    r
}
