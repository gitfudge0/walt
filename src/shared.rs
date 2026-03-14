use std::{collections::HashSet, path::PathBuf};

use crate::{
    backend::{Monitor, RandomMode, RandomPlan},
    cache::IndexedWallpaper,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DisplayTarget {
    Monitor(String),
    AllDisplays,
}

impl DisplayTarget {
    pub fn label(&self) -> &str {
        match self {
            Self::Monitor(name) => name,
            Self::AllDisplays => "All displays",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WallpaperApplyAction {
    ErrorNoMonitors,
    ApplyToSingleDisplay(String),
    OpenDisplayPicker(Vec<String>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RandomMenuAction {
    DifferentAll,
    SameAll,
    DisplayIndex(usize, String),
}

impl RandomMenuAction {
    pub fn label(&self) -> String {
        match self {
            Self::DifferentAll => "Different on all displays".to_string(),
            Self::SameAll => "Same on all displays".to_string(),
            Self::DisplayIndex(index, name) => format!("Display {index}: {name}"),
        }
    }

    pub fn mode(&self) -> RandomMode {
        match self {
            Self::DifferentAll => RandomMode::DifferentAll,
            Self::SameAll => RandomMode::SameAll,
            Self::DisplayIndex(index, _) => RandomMode::DisplayIndex(*index),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RandomApplyAction {
    ErrorNoMonitors,
    ApplyToSingleDisplay,
    OpenRandomMenu(Vec<String>),
}

pub fn display_targets_from_names(monitor_names: &[String]) -> Vec<DisplayTarget> {
    let mut targets = monitor_names
        .iter()
        .cloned()
        .map(DisplayTarget::Monitor)
        .collect::<Vec<_>>();
    targets.push(DisplayTarget::AllDisplays);
    targets
}

pub fn default_display_target_selection(targets: &[DisplayTarget]) -> Option<usize> {
    targets
        .iter()
        .position(|target| matches!(target, DisplayTarget::Monitor(_)))
}

pub fn wallpaper_apply_action(monitors: &[Monitor]) -> WallpaperApplyAction {
    match monitors {
        [] => WallpaperApplyAction::ErrorNoMonitors,
        [monitor] => WallpaperApplyAction::ApplyToSingleDisplay(monitor.name.clone()),
        _ => WallpaperApplyAction::OpenDisplayPicker(
            monitors
                .iter()
                .map(|monitor| monitor.name.clone())
                .collect(),
        ),
    }
}

pub fn random_apply_action(monitors: &[Monitor]) -> RandomApplyAction {
    match monitors {
        [] => RandomApplyAction::ErrorNoMonitors,
        [_monitor] => RandomApplyAction::ApplyToSingleDisplay,
        _ => RandomApplyAction::OpenRandomMenu(
            monitors
                .iter()
                .map(|monitor| monitor.name.clone())
                .collect(),
        ),
    }
}

pub fn random_menu_actions(monitor_names: &[String]) -> Vec<RandomMenuAction> {
    let mut actions = vec![RandomMenuAction::DifferentAll, RandomMenuAction::SameAll];
    actions.extend(
        monitor_names
            .iter()
            .enumerate()
            .map(|(index, name)| RandomMenuAction::DisplayIndex(index, name.clone())),
    );
    actions
}

pub fn first_active_visible_index(
    indices: &[usize],
    wallpapers: &[IndexedWallpaper],
    active_wallpaper_paths: &HashSet<PathBuf>,
) -> Option<usize> {
    indices.iter().position(|index| {
        wallpapers
            .get(*index)
            .map(|wallpaper| active_wallpaper_paths.contains(&wallpaper.path))
            .unwrap_or(false)
    })
}

pub fn selection_for_random_plan(
    indices: &[usize],
    wallpapers: &[IndexedWallpaper],
    plan: &RandomPlan,
) -> Option<usize> {
    let mut unique_paths = HashSet::new();
    for assignment in &plan.assignments {
        unique_paths.insert(assignment.wallpaper_path.clone());
    }

    if unique_paths.len() != 1 {
        return None;
    }

    let path = unique_paths.into_iter().next()?;
    indices.iter().position(|index| {
        wallpapers
            .get(*index)
            .map(|wallpaper| wallpaper.path == path)
            .unwrap_or(false)
    })
}

pub fn set_active_wallpaper_paths<I>(active_wallpaper_paths: &mut HashSet<PathBuf>, paths: I)
where
    I: IntoIterator<Item = PathBuf>,
{
    *active_wallpaper_paths = paths.into_iter().collect();
}

pub fn mark_active_wallpaper(
    active_wallpaper_paths: &mut HashSet<PathBuf>,
    wallpaper_path: &PathBuf,
) {
    active_wallpaper_paths.insert(wallpaper_path.clone());
}

pub fn active_wallpaper_paths_for_random_plan(plan: &RandomPlan) -> HashSet<PathBuf> {
    plan.assignments
        .iter()
        .map(|assignment| assignment.wallpaper_path.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{collections::HashSet, path::PathBuf};

    use super::{
        active_wallpaper_paths_for_random_plan, default_display_target_selection,
        display_targets_from_names, first_active_visible_index, mark_active_wallpaper,
        random_apply_action, random_menu_actions, selection_for_random_plan,
        set_active_wallpaper_paths, wallpaper_apply_action, DisplayTarget, RandomApplyAction,
        RandomMenuAction, WallpaperApplyAction,
    };
    use crate::{
        backend::random::RandomAssignment,
        backend::{Monitor, RandomMode, RandomPlan},
        cache::IndexedWallpaper,
    };

    fn wallpaper(name: &str) -> IndexedWallpaper {
        IndexedWallpaper {
            path: PathBuf::from(format!("/wallpapers/{name}.jpg")),
            name: name.to_string(),
            directory: PathBuf::from("/wallpapers"),
            extension: "jpg".to_string(),
            modified_unix_secs: 0,
            file_size: 0,
            width: None,
            height: None,
        }
    }

    #[test]
    fn display_targets_append_all_displays() {
        let targets = display_targets_from_names(&["HDMI-A-1".to_string(), "DP-1".to_string()]);

        assert_eq!(
            targets,
            vec![
                DisplayTarget::Monitor("HDMI-A-1".to_string()),
                DisplayTarget::Monitor("DP-1".to_string()),
                DisplayTarget::AllDisplays,
            ]
        );
    }

    #[test]
    fn defaults_display_target_selection_to_first_monitor() {
        let targets = vec![
            DisplayTarget::Monitor("HDMI-A-1".to_string()),
            DisplayTarget::Monitor("DP-1".to_string()),
            DisplayTarget::AllDisplays,
        ];

        assert_eq!(default_display_target_selection(&targets), Some(0));
    }

    #[test]
    fn random_menu_actions_include_all_modes_in_order() {
        let actions = random_menu_actions(&["HDMI-A-1".to_string(), "DP-1".to_string()]);

        assert_eq!(
            actions,
            vec![
                RandomMenuAction::DifferentAll,
                RandomMenuAction::SameAll,
                RandomMenuAction::DisplayIndex(0, "HDMI-A-1".to_string()),
                RandomMenuAction::DisplayIndex(1, "DP-1".to_string()),
            ]
        );
    }

    #[test]
    fn random_apply_action_applies_directly_for_one_monitor() {
        assert_eq!(
            random_apply_action(&[Monitor {
                name: "HDMI-A-1".to_string()
            }]),
            RandomApplyAction::ApplyToSingleDisplay
        );
    }

    #[test]
    fn random_apply_action_opens_menu_for_multiple_monitors() {
        assert_eq!(
            random_apply_action(&[
                Monitor {
                    name: "HDMI-A-1".to_string()
                },
                Monitor {
                    name: "DP-1".to_string()
                }
            ]),
            RandomApplyAction::OpenRandomMenu(vec!["HDMI-A-1".to_string(), "DP-1".to_string()])
        );
    }

    #[test]
    fn wallpaper_apply_action_errors_without_monitors() {
        assert_eq!(
            wallpaper_apply_action(&[]),
            WallpaperApplyAction::ErrorNoMonitors
        );
    }

    #[test]
    fn wallpaper_apply_action_applies_directly_for_one_monitor() {
        assert_eq!(
            wallpaper_apply_action(&[Monitor {
                name: "HDMI-A-1".to_string()
            }]),
            WallpaperApplyAction::ApplyToSingleDisplay("HDMI-A-1".to_string())
        );
    }

    #[test]
    fn wallpaper_apply_action_opens_picker_for_multiple_monitors() {
        assert_eq!(
            wallpaper_apply_action(&[
                Monitor {
                    name: "HDMI-A-1".to_string()
                },
                Monitor {
                    name: "DP-1".to_string()
                }
            ]),
            WallpaperApplyAction::OpenDisplayPicker(vec![
                "HDMI-A-1".to_string(),
                "DP-1".to_string()
            ])
        );
    }

    #[test]
    fn selects_first_active_wallpaper_in_all() {
        let wallpapers = vec![wallpaper("alpha"), wallpaper("beta"), wallpaper("gamma")];
        let indices = vec![0, 1, 2];
        let active_paths = HashSet::from([wallpapers[1].path.clone()]);

        assert_eq!(
            first_active_visible_index(&indices, &wallpapers, &active_paths),
            Some(1)
        );
    }

    #[test]
    fn syncs_selection_to_single_random_path() {
        let wallpapers = vec![wallpaper("alpha"), wallpaper("beta"), wallpaper("gamma")];
        let indices = vec![2, 1, 0];
        let plan = RandomPlan {
            mode: RandomMode::DisplayIndex(0),
            assignments: vec![RandomAssignment {
                monitor_name: "HDMI-A-1".to_string(),
                wallpaper_path: wallpapers[2].path.clone(),
            }],
            requested_display_index: Some(0),
            resolved_display_index: Some(0),
        };

        assert_eq!(
            selection_for_random_plan(&indices, &wallpapers, &plan),
            Some(0)
        );
    }

    #[test]
    fn ignores_multi_path_random_plans_for_selection_sync() {
        let wallpapers = vec![wallpaper("alpha"), wallpaper("beta"), wallpaper("gamma")];
        let indices = vec![0, 1, 2];
        let plan = RandomPlan {
            mode: RandomMode::DifferentAll,
            assignments: vec![
                RandomAssignment {
                    monitor_name: "HDMI-A-1".to_string(),
                    wallpaper_path: wallpapers[0].path.clone(),
                },
                RandomAssignment {
                    monitor_name: "DP-1".to_string(),
                    wallpaper_path: wallpapers[1].path.clone(),
                },
            ],
            requested_display_index: None,
            resolved_display_index: None,
        };

        assert_eq!(
            selection_for_random_plan(&indices, &wallpapers, &plan),
            None
        );
    }

    #[test]
    fn set_active_wallpaper_paths_replaces_existing_state() {
        let mut active_paths = HashSet::from([
            PathBuf::from("/wallpapers/old-alpha.jpg"),
            PathBuf::from("/wallpapers/old-beta.jpg"),
        ]);

        set_active_wallpaper_paths(
            &mut active_paths,
            vec![PathBuf::from("/wallpapers/new-alpha.jpg")],
        );

        assert_eq!(
            active_paths,
            HashSet::from([PathBuf::from("/wallpapers/new-alpha.jpg")])
        );
    }

    #[test]
    fn mark_active_wallpaper_preserves_existing_state() {
        let mut active_paths = HashSet::from([PathBuf::from("/wallpapers/alpha.jpg")]);

        mark_active_wallpaper(&mut active_paths, &PathBuf::from("/wallpapers/beta.jpg"));

        assert_eq!(
            active_paths,
            HashSet::from([
                PathBuf::from("/wallpapers/alpha.jpg"),
                PathBuf::from("/wallpapers/beta.jpg"),
            ])
        );
    }

    #[test]
    fn active_wallpaper_paths_for_random_plan_deduplicates_paths() {
        let plan = RandomPlan {
            mode: RandomMode::SameAll,
            assignments: vec![
                RandomAssignment {
                    monitor_name: "HDMI-A-1".to_string(),
                    wallpaper_path: PathBuf::from("/wallpapers/alpha.jpg"),
                },
                RandomAssignment {
                    monitor_name: "DP-1".to_string(),
                    wallpaper_path: PathBuf::from("/wallpapers/alpha.jpg"),
                },
            ],
            requested_display_index: None,
            resolved_display_index: None,
        };

        assert_eq!(
            active_wallpaper_paths_for_random_plan(&plan),
            HashSet::from([PathBuf::from("/wallpapers/alpha.jpg")])
        );
    }
}
