pub fn cycle_index(active: Option<usize>, tab_count: usize, delta: i32) -> Option<usize> {
    if tab_count == 0 {
        return None;
    }

    let active = active.filter(|index| *index < tab_count).unwrap_or(0);
    Some((active as i64 + i64::from(delta)).rem_euclid(tab_count as i64) as usize)
}

pub type TabId = u32;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TabGroups {
    placements: Vec<(TabId, usize)>,
    active: [Option<TabId>; 2],
    focused_group: usize,
}

impl TabGroups {
    pub fn add(&mut self, id: TabId, group: usize) {
        let group = group.min(1);
        self.placements.push((id, group));
        self.active[group] = Some(id);
        self.focused_group = group;
    }

    pub fn activate(&mut self, id: TabId) -> bool {
        let Some(group) = self.group_of(id) else {
            return false;
        };
        self.active[group] = Some(id);
        self.focused_group = group;
        true
    }

    pub fn remove(&mut self, id: TabId) -> bool {
        let Some(index) = self.placements.iter().position(|(tab_id, _)| *tab_id == id) else {
            return false;
        };
        let removed_group = self.placements[index].1;
        self.placements.remove(index);
        if self.active[removed_group] == Some(id) {
            self.active[removed_group] = self
                .placements
                .iter()
                .rev()
                .find_map(|(tab_id, group)| (*group == removed_group).then_some(*tab_id));
        }
        self.normalize_primary_group();
        true
    }

    pub fn dock(&mut self, id: TabId, zone: i32) -> bool {
        let Some(source_group) = self.group_of(id) else {
            return false;
        };
        // Edge zones 0..=3 use the secondary pane. Body zones 4 and 5
        // explicitly target the primary and secondary pane, respectively.
        let target_group = if zone == 4 || self.placements.len() < 2 {
            0
        } else {
            1
        };

        if source_group == 0
            && target_group == 1
            && zone != 5
            && self.group_ids(0).len() == 1
            && self.has_secondary()
        {
            for (tab_id, group) in &mut self.placements {
                if *tab_id != id && *group == 1 {
                    *group = 0;
                }
            }
            self.active[0] = self.active[1].filter(|active| *active != id);
            self.active[1] = None;
        }

        if let Some((_, group)) = self
            .placements
            .iter_mut()
            .find(|(tab_id, _)| *tab_id == id)
        {
            *group = target_group;
        }
        if self.active[source_group] == Some(id) && source_group != target_group {
            self.active[source_group] = self
                .placements
                .iter()
                .rev()
                .find_map(|(tab_id, group)| (*group == source_group).then_some(*tab_id));
        }
        self.active[target_group] = Some(id);
        self.focused_group = target_group;
        self.normalize_primary_group();
        true
    }

    pub fn cycle(&mut self, delta: i32) -> Option<TabId> {
        let ids = self.group_ids(self.focused_group);
        let active_index = self.active[self.focused_group]
            .and_then(|active| ids.iter().position(|id| *id == active));
        let next = cycle_index(active_index, ids.len(), delta).map(|index| ids[index]);
        self.active[self.focused_group] = next;
        next
    }

    pub fn set_focused_group(&mut self, group: usize) {
        if group < 2 && self.active[group].is_some() {
            self.focused_group = group;
        }
    }

    pub fn focused_group(&self) -> usize {
        self.focused_group
    }

    pub fn active(&self, group: usize) -> Option<TabId> {
        self.active.get(group).copied().flatten()
    }

    pub fn group_of(&self, id: TabId) -> Option<usize> {
        self.placements
            .iter()
            .find_map(|(tab_id, group)| (*tab_id == id).then_some(*group))
    }

    pub fn group_ids(&self, group: usize) -> Vec<TabId> {
        self.placements
            .iter()
            .filter_map(|(id, tab_group)| (*tab_group == group).then_some(*id))
            .collect()
    }

    pub fn has_secondary(&self) -> bool {
        self.placements.iter().any(|(_, group)| *group == 1)
    }

    fn normalize_primary_group(&mut self) {
        if self.active[0].is_none() && self.has_secondary() {
            for (_, group) in &mut self.placements {
                if *group == 1 {
                    *group = 0;
                }
            }
            self.active[0] = self.active[1];
            self.active[1] = None;
            self.focused_group = 0;
        }
        if self.active[self.focused_group].is_none() {
            self.focused_group = if self.active[0].is_some() { 0 } else { 1 };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_tabs_in_both_directions_and_wraps() {
        assert_eq!(cycle_index(None, 0, 1), None);
        assert_eq!(cycle_index(Some(0), 4, 1), Some(1));
        assert_eq!(cycle_index(Some(3), 4, 1), Some(0));
        assert_eq!(cycle_index(Some(0), 4, -1), Some(3));
    }

    #[test]
    fn manages_two_groups_with_stable_ids() {
        let mut groups = TabGroups::default();
        groups.add(10, 0);
        groups.add(20, 0);
        groups.add(30, 0);
        assert_eq!(groups.active(0), Some(30));

        assert!(groups.dock(20, 1));
        assert_eq!(groups.group_ids(0), vec![10, 30]);
        assert_eq!(groups.group_ids(1), vec![20]);
        assert_eq!(groups.active(1), Some(20));

        groups.set_focused_group(0);
        assert_eq!(groups.cycle(1), Some(10));
        assert!(groups.activate(30));
        assert_eq!(groups.focused_group(), 0);
        assert!(groups.remove(30));
        assert_eq!(groups.active(0), Some(10));
    }

    #[test]
    fn docking_last_primary_tab_swaps_groups_without_losing_a_pane() {
        let mut groups = TabGroups::default();
        groups.add(1, 0);
        groups.add(2, 1);
        groups.add(3, 1);

        assert!(groups.dock(1, 0));
        assert_eq!(groups.group_ids(0), vec![2, 3]);
        assert_eq!(groups.group_ids(1), vec![1]);
        assert_eq!(groups.active(0), Some(3));
        assert_eq!(groups.active(1), Some(1));

        assert!(groups.remove(1));
        assert!(!groups.has_secondary());
        assert_eq!(groups.group_ids(0), vec![2, 3]);
    }

    #[test]
    fn center_drop_returns_tabs_to_primary_group() {
        let mut groups = TabGroups::default();
        groups.add(1, 0);
        groups.add(2, 0);
        groups.dock(2, 3);
        assert!(groups.has_secondary());
        groups.dock(2, 4);
        assert!(!groups.has_secondary());
        assert_eq!(groups.active(0), Some(2));
    }

    #[test]
    fn dropping_the_last_primary_tab_onto_secondary_merges_the_panes() {
        let mut groups = TabGroups::default();
        groups.add(1, 0);
        groups.add(2, 1);
        groups.add(3, 1);

        assert!(groups.dock(1, 5));
        assert!(!groups.has_secondary());
        assert_eq!(groups.group_ids(0), vec![1, 2, 3]);
        assert_eq!(groups.active(0), Some(1));
        assert_eq!(groups.focused_group(), 0);
    }
}
