pub mod rpgmaker_mv;
pub mod rpgmaker_vxa;
pub mod renpy;
pub mod wolf_rpg;
pub mod sugarcube;
pub mod unreal;

use locust_core::extraction::FormatRegistry;

pub fn default_registry() -> FormatRegistry {
    let mut r = FormatRegistry::new();
    r.register(Box::new(rpgmaker_mv::RpgMakerMvPlugin::new()));
    r.register(Box::new(rpgmaker_vxa::RpgMakerVxaPlugin::new()));
    r.register(Box::new(renpy::RenPyPlugin::new()));
    r.register(Box::new(wolf_rpg::WolfRpgPlugin::new()));
    r.register(Box::new(sugarcube::SugarCubePlugin::new()));
    r.register(Box::new(unreal::UnrealPlugin::new()));
    r
}
