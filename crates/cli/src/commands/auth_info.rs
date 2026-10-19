use comfy_table::Table;
use locust_core::config::AppConfig;
use locust_core::models::OutputMode;

pub(crate) async fn cmd_auth(provider: String) -> anyhow::Result<()> {
    match provider.as_str() {
        "grok" | "grok-sub" | "xai" => {
            locust_providers::xai_oauth::device_login().await?;
            println!("\nLogged in to xAI.");
            println!("Translate with your subscription: locust translate <db> -p grok-sub");
        }
        other => anyhow::bail!("unknown auth provider: {}. Available: grok", other),
    }
    Ok(())
}

pub(crate) fn cmd_providers(config: &AppConfig) -> anyhow::Result<()> {
    let reg = locust_providers::default_registry(config);
    let providers = reg.list();

    let mut table = Table::new();
    table.set_header(vec!["ID", "Name", "Free", "Requires API Key"]);
    for p in &providers {
        table.add_row(vec![
            p.id.clone(),
            p.name.clone(),
            if p.is_free {
                "Yes".to_string()
            } else {
                "No".to_string()
            },
            if p.requires_api_key {
                "Yes".to_string()
            } else {
                "No".to_string()
            },
        ]);
    }
    println!("{table}");

    Ok(())
}

pub(crate) fn cmd_formats() -> anyhow::Result<()> {
    let registry = locust_formats::default_registry();
    let formats = registry.list();

    let mut table = Table::new();
    table.set_header(vec!["ID", "Name", "Extensions", "Modes", "Stability"]);
    for f in &formats {
        let modes: Vec<&str> = f
            .supported_modes
            .iter()
            .map(|m| match m {
                OutputMode::Replace => "Replace",
                OutputMode::Add => "Add",
            })
            .collect();
        table.add_row(vec![
            &f.id,
            &f.name,
            &f.extensions.join(", "),
            &modes.join(", "),
            f.stability.label(),
        ]);
    }
    println!("{table}");

    Ok(())
}
