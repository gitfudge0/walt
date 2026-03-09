use std::path::PathBuf;

use anyhow::{bail, Result};
use rand::{seq::SliceRandom, Rng};

use super::{set_wallpaper, set_wallpaper_for_monitor, set_wallpapers_for_monitors, Monitor};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RandomMode {
    DifferentAll,
    SameAll,
    DisplayIndex(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RandomAssignment {
    pub monitor_name: String,
    pub wallpaper_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RandomPlan {
    pub mode: RandomMode,
    pub assignments: Vec<RandomAssignment>,
    pub requested_display_index: Option<usize>,
    pub resolved_display_index: Option<usize>,
}

pub fn apply_random_plan(plan: &RandomPlan) -> Result<()> {
    match &plan.mode {
        RandomMode::SameAll => {
            let wallpaper = plan
                .assignments
                .first()
                .ok_or_else(|| anyhow::anyhow!("No random wallpaper assignment was generated"))?;
            set_wallpaper(&wallpaper.wallpaper_path.to_string_lossy())
        }
        RandomMode::DisplayIndex(_) => {
            let assignment = plan
                .assignments
                .first()
                .ok_or_else(|| anyhow::anyhow!("No random wallpaper assignment was generated"))?;
            set_wallpaper_for_monitor(
                &assignment.monitor_name,
                &assignment.wallpaper_path.to_string_lossy(),
            )
        }
        RandomMode::DifferentAll => {
            let assignments = plan
                .assignments
                .iter()
                .map(|assignment| {
                    (
                        assignment.monitor_name.clone(),
                        assignment.wallpaper_path.clone(),
                    )
                })
                .collect::<Vec<_>>();
            set_wallpapers_for_monitors(&assignments)
        }
    }
}

pub fn plan_random_assignments(
    monitors: &[Monitor],
    wallpapers: &[PathBuf],
    mode: RandomMode,
) -> Result<RandomPlan> {
    let mut rng = rand::thread_rng();
    plan_random_assignments_with_rng(monitors, wallpapers, mode, &mut rng)
}

pub fn plan_random_assignments_with_rng<R: Rng + ?Sized>(
    monitors: &[Monitor],
    wallpapers: &[PathBuf],
    mode: RandomMode,
    rng: &mut R,
) -> Result<RandomPlan> {
    if monitors.is_empty() {
        bail!("No monitors found");
    }

    if wallpapers.is_empty() {
        bail!("No wallpapers found in configured directories.");
    }

    match mode.clone() {
        RandomMode::DifferentAll => {
            let selected = choose_distinct_then_repeat(wallpapers, monitors.len(), rng);
            let assignments = monitors
                .iter()
                .zip(selected)
                .map(|(monitor, wallpaper_path)| RandomAssignment {
                    monitor_name: monitor.name.clone(),
                    wallpaper_path,
                })
                .collect::<Vec<_>>();

            Ok(RandomPlan {
                mode,
                assignments,
                requested_display_index: None,
                resolved_display_index: None,
            })
        }
        RandomMode::SameAll => {
            let wallpaper_path = wallpapers
                .choose(rng)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("No wallpapers found in configured directories."))?;
            let assignments = monitors
                .iter()
                .map(|monitor| RandomAssignment {
                    monitor_name: monitor.name.clone(),
                    wallpaper_path: wallpaper_path.clone(),
                })
                .collect::<Vec<_>>();

            Ok(RandomPlan {
                mode,
                assignments,
                requested_display_index: None,
                resolved_display_index: None,
            })
        }
        RandomMode::DisplayIndex(requested) => {
            let resolved = resolve_display_index(requested, monitors.len());
            let wallpaper_path = wallpapers
                .choose(rng)
                .cloned()
                .ok_or_else(|| anyhow::anyhow!("No wallpapers found in configured directories."))?;
            let monitor = monitors
                .get(resolved)
                .ok_or_else(|| anyhow::anyhow!("No monitors found"))?;

            Ok(RandomPlan {
                mode,
                assignments: vec![RandomAssignment {
                    monitor_name: monitor.name.clone(),
                    wallpaper_path,
                }],
                requested_display_index: Some(requested),
                resolved_display_index: Some(resolved),
            })
        }
    }
}

pub fn resolve_display_index(requested: usize, monitor_count: usize) -> usize {
    requested.min(monitor_count.saturating_sub(1))
}

pub fn choose_distinct_then_repeat<R: Rng + ?Sized>(
    wallpapers: &[PathBuf],
    count: usize,
    rng: &mut R,
) -> Vec<PathBuf> {
    if wallpapers.is_empty() || count == 0 {
        return vec![];
    }

    let mut shuffled = wallpapers.to_vec();
    shuffled.shuffle(rng);

    let mut selected = Vec::with_capacity(count);
    while selected.len() < count {
        let next = &shuffled[selected.len() % shuffled.len()];
        selected.push(next.clone());
    }

    selected
}

#[cfg(test)]
mod tests {
    use super::{
        choose_distinct_then_repeat, plan_random_assignments_with_rng, resolve_display_index,
        RandomMode,
    };
    use crate::backend::Monitor;
    use rand::rngs::StdRng;
    use rand::SeedableRng;
    use std::path::PathBuf;

    fn monitors(names: &[&str]) -> Vec<Monitor> {
        names
            .iter()
            .map(|name| Monitor {
                name: (*name).to_string(),
            })
            .collect()
    }

    fn wallpapers(names: &[&str]) -> Vec<PathBuf> {
        names
            .iter()
            .map(|name| PathBuf::from(format!("/wallpapers/{name}.jpg")))
            .collect()
    }

    #[test]
    fn resolves_display_index_to_last_display_when_out_of_range() {
        assert_eq!(resolve_display_index(99, 1), 0);
        assert_eq!(resolve_display_index(99, 3), 2);
    }

    #[test]
    fn chooses_one_assignment_for_one_monitor_in_different_all_mode() {
        let mut rng = StdRng::seed_from_u64(7);
        let plan = plan_random_assignments_with_rng(
            &monitors(&["HDMI-A-1"]),
            &wallpapers(&["alpha", "beta"]),
            RandomMode::DifferentAll,
            &mut rng,
        )
        .expect("plan");

        assert_eq!(plan.assignments.len(), 1);
    }

    #[test]
    fn chooses_one_assignment_for_one_monitor_in_same_all_mode() {
        let mut rng = StdRng::seed_from_u64(7);
        let plan = plan_random_assignments_with_rng(
            &monitors(&["HDMI-A-1"]),
            &wallpapers(&["alpha", "beta"]),
            RandomMode::SameAll,
            &mut rng,
        )
        .expect("plan");

        assert_eq!(plan.assignments.len(), 1);
    }

    #[test]
    fn clamps_display_index_when_planning_single_display_random() {
        let mut rng = StdRng::seed_from_u64(7);
        let plan = plan_random_assignments_with_rng(
            &monitors(&["HDMI-A-1"]),
            &wallpapers(&["alpha", "beta"]),
            RandomMode::DisplayIndex(99),
            &mut rng,
        )
        .expect("plan");

        assert_eq!(plan.resolved_display_index, Some(0));
        assert_eq!(plan.assignments[0].monitor_name, "HDMI-A-1");
    }

    #[test]
    fn different_all_uses_distinct_wallpapers_when_possible() {
        let mut rng = StdRng::seed_from_u64(7);
        let plan = plan_random_assignments_with_rng(
            &monitors(&["HDMI-A-1", "DP-1"]),
            &wallpapers(&["alpha", "beta", "gamma"]),
            RandomMode::DifferentAll,
            &mut rng,
        )
        .expect("plan");

        assert_eq!(plan.assignments.len(), 2);
        assert_ne!(
            plan.assignments[0].wallpaper_path,
            plan.assignments[1].wallpaper_path
        );
    }

    #[test]
    fn different_all_reuses_only_after_unique_pool_is_exhausted() {
        let mut rng = StdRng::seed_from_u64(7);
        let picked = choose_distinct_then_repeat(&wallpapers(&["alpha", "beta"]), 3, &mut rng);

        assert_ne!(picked[0], picked[1]);
        assert!(picked[2] == picked[0] || picked[2] == picked[1]);
    }

    #[test]
    fn same_all_assigns_identical_wallpaper_to_every_monitor() {
        let mut rng = StdRng::seed_from_u64(7);
        let plan = plan_random_assignments_with_rng(
            &monitors(&["HDMI-A-1", "DP-1"]),
            &wallpapers(&["alpha", "beta", "gamma"]),
            RandomMode::SameAll,
            &mut rng,
        )
        .expect("plan");

        assert_eq!(plan.assignments.len(), 2);
        assert_eq!(
            plan.assignments[0].wallpaper_path,
            plan.assignments[1].wallpaper_path
        );
    }

    #[test]
    fn returns_error_when_no_monitors_are_available() {
        let mut rng = StdRng::seed_from_u64(7);
        let error = plan_random_assignments_with_rng(
            &[],
            &wallpapers(&["alpha"]),
            RandomMode::DifferentAll,
            &mut rng,
        )
        .expect_err("should fail");

        assert_eq!(error.to_string(), "No monitors found");
    }

    #[test]
    fn returns_error_when_no_wallpapers_are_available() {
        let mut rng = StdRng::seed_from_u64(7);
        let error = plan_random_assignments_with_rng(
            &monitors(&["HDMI-A-1"]),
            &[],
            RandomMode::DifferentAll,
            &mut rng,
        )
        .expect_err("should fail");

        assert_eq!(
            error.to_string(),
            "No wallpapers found in configured directories."
        );
    }
}
