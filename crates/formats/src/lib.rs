pub mod binary_search;
pub mod rpgmaker_mv;
pub mod rpgmaker_vxa;
pub mod renpy;
pub mod wolf_rpg;
pub mod sugarcube;
pub mod unreal;
pub mod unity;
pub mod html_game;
pub mod vntextpatch;
pub mod qsp;
// tyrano before kirikiri: both may see loose .ks; Tyrano claims data/scenario/ + tyrano/ trees.
pub mod tyrano;
pub mod tyrano_asar;
pub mod kirikiri;
pub mod kirikiri_xp3;
pub mod yuris;
pub mod yuris_ypf;
pub mod nscripter;

use locust_core::extraction::FormatRegistry;

pub fn default_registry() -> FormatRegistry {
    let mut r = FormatRegistry::new();
    r.register(Box::new(rpgmaker_mv::RpgMakerMvPlugin::new()));
    r.register(Box::new(rpgmaker_vxa::RpgMakerVxaPlugin::new()));
    r.register(Box::new(renpy::RenPyPlugin::new()));
    r.register(Box::new(wolf_rpg::WolfRpgPlugin::new()));
    r.register(Box::new(sugarcube::SugarCubePlugin::new()));
    r.register(Box::new(unreal::UnrealPlugin::new()));
    r.register(Box::new(unity::UnityPlugin::new()));
    // html-game must be AFTER sugarcube (more specific wins first)
    r.register(Box::new(html_game::HtmlGamePlugin::new()));
    r.register(Box::new(qsp::QspPlugin::new()));
    // tyrano before kirikiri so TyranoBuilder dirs are not claimed as KiriKiri.
    r.register(Box::new(tyrano::TyranoPlugin::new()));
    r.register(Box::new(kirikiri::KirikiriPlugin::new()));
    r.register(Box::new(yuris::YurisPlugin::new()));
    r.register(Box::new(nscripter::NScripterPlugin::new()));
    // vntextpatch last: only claims folders of {"message":...} JSON, so it
    // never shadows a real game format detected above.
    r.register(Box::new(vntextpatch::VnTextPatchPlugin::new()));
    r
}
