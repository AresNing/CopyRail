use serde::Serialize;

/// Only public build metadata. No user paths, history, account or permissions.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BuildInfo {
    format_version: u8,
    app_identifier: String,
    product_name: Option<String>,
    version: &'static str,
    debug_build: bool,
    cloud_transport_compiled: bool,
    main_window_title: Option<String>,
}

impl BuildInfo {
    pub(crate) fn from_config(config: &tauri::Config) -> Self {
        Self {
            format_version: 1,
            app_identifier: config.identifier.clone(),
            product_name: config.product_name.clone(),
            version: env!("CARGO_PKG_VERSION"),
            debug_build: cfg!(debug_assertions),
            cloud_transport_compiled: cfg!(feature = "cloudkit"),
            main_window_title: config
                .app
                .windows
                .iter()
                .find(|window| window.label == "main")
                .map(|window| window.title.clone()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_uses_effective_configuration_without_starting_services() {
        let config: tauri::Config = serde_json::from_value(serde_json::json!({
            "identifier": "io.pasters.localqa.example",
            "productName": "CopyRail Local QA Example",
            "app": {"windows": [{"label": "main", "title": "Core acceptance"}]}
        }))
        .expect("valid synthetic Tauri configuration");
        let json = serde_json::to_value(BuildInfo::from_config(&config))
            .expect("public build metadata serializes");
        assert_eq!(json["appIdentifier"], "io.pasters.localqa.example");
        assert_eq!(json["productName"], "CopyRail Local QA Example");
        assert_eq!(json["mainWindowTitle"], "Core acceptance");
        assert_eq!(json["cloudTransportCompiled"], cfg!(feature = "cloudkit"));
        assert_eq!(json["debugBuild"], cfg!(debug_assertions));
        assert_eq!(json["formatVersion"], 1);
        assert_eq!(json.as_object().expect("metadata is an object").len(), 7);
    }
}
