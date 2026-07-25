pub mod visual_diff;
pub mod visual_scenario;

#[cfg(test)]
mod tests {
    use std::{fs, time::Duration};

    use mandatum_scene::OverlayScene;

    use crate::visual_scenario::{VisualScenarioId, prepare_visual_scenario};

    #[test]
    fn catalog_exposes_the_canonical_scenarios_in_review_order() {
        let ids = VisualScenarioId::ALL
            .iter()
            .map(|scenario| scenario.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            [
                "typography",
                "calm-terminal",
                "dense-workspace",
                "attention",
                "palette",
                "full-modal",
                "welcome",
                "context-menu",
                "artifacts",
                "narrow",
                "restored",
            ]
        );
    }

    #[test]
    fn palette_scenario_is_driven_through_real_host_and_neutral_input() {
        let root = std::env::temp_dir().join(format!(
            "mandatum-spike-visual-palette-{}",
            std::process::id()
        ));
        let scenario =
            prepare_visual_scenario(VisualScenarioId::Palette, &root).expect("prepare scenario");
        let mut host = mandatum_app::FrontendHost::new(scenario.app_config());
        let snapshot = scenario
            .drive(
                &mut host,
                mandatum_scene::SceneSize::new(102, 35),
                Duration::from_secs(2),
            )
            .expect("scenario frame");

        assert!(matches!(
            snapshot.scene.overlay,
            Some(OverlayScene::Palette(_))
        ));
        host.shutdown();
        let _ = fs::remove_dir_all(root);
    }
}
