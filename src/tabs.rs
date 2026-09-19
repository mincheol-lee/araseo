pub type TabId = u32;
pub type GroupId = usize;

pub fn cycle_index(active: Option<usize>, count: usize, delta: i32) -> Option<usize> {
    (count > 0).then(|| {
        (active.filter(|i| *i < count).unwrap_or(0) as i64 + i64::from(delta))
            .rem_euclid(count as i64) as usize
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dock {
    Left,
    Right,
    Top,
    Bottom,
    Center,
}

impl Dock {
    pub fn from_zone(zone: i32) -> Option<Self> {
        match zone {
            0 => Some(Self::Left),
            1 => Some(Self::Right),
            2 => Some(Self::Top),
            3 => Some(Self::Bottom),
            4 => Some(Self::Center),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl Rect {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.width && y < self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Divider {
    pub id: usize,
    pub axis: Axis,
    pub rect: Rect,
    pub parent: Rect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Node {
    Leaf(GroupId),
    Split {
        id: usize,
        axis: Axis,
        ratio: u16,
        first: Box<Node>,
        second: Box<Node>,
    },
}

impl Node {
    fn has(&self, id: GroupId) -> bool {
        match self {
            Self::Leaf(group) => *group == id,
            Self::Split { first, second, .. } => first.has(id) || second.has(id),
        }
    }

    fn split(&mut self, target: GroupId, new_group: GroupId, dock: Dock, id: usize) -> bool {
        match self {
            Self::Leaf(group) if *group == target => {
                let axis = match dock {
                    Dock::Left | Dock::Right => Axis::Horizontal,
                    Dock::Top | Dock::Bottom => Axis::Vertical,
                    Dock::Center => return false,
                };
                let new_first = matches!(dock, Dock::Left | Dock::Top);
                *self = Self::Split {
                    id,
                    axis,
                    ratio: 500,
                    first: Box::new(Self::Leaf(if new_first { new_group } else { target })),
                    second: Box::new(Self::Leaf(if new_first { target } else { new_group })),
                };
                true
            }
            Self::Split { first, second, .. } => {
                first.split(target, new_group, dock, id)
                    || second.split(target, new_group, dock, id)
            }
            _ => false,
        }
    }

    fn remove(&mut self, group: GroupId) -> bool {
        let Self::Split { first, second, .. } = self else {
            return false;
        };
        if matches!(first.as_ref(), Self::Leaf(id) if *id == group) {
            *self = *second.clone();
            true
        } else if matches!(second.as_ref(), Self::Leaf(id) if *id == group) {
            *self = *first.clone();
            true
        } else {
            first.remove(group) || second.remove(group)
        }
    }

    fn visit(&self, rect: Rect, panes: &mut Vec<(GroupId, Rect)>, dividers: &mut Vec<Divider>) {
        match self {
            Self::Leaf(group) => panes.push((*group, rect)),
            Self::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => {
                let r = f32::from(*ratio) / 1000.0;
                let (a, b, divider) = match axis {
                    Axis::Horizontal => {
                        let width = rect.width * r;
                        (
                            Rect { width, ..rect },
                            Rect {
                                x: rect.x + width,
                                width: rect.width - width,
                                ..rect
                            },
                            Rect {
                                x: rect.x + width,
                                width: 0.0,
                                ..rect
                            },
                        )
                    }
                    Axis::Vertical => {
                        let height = rect.height * r;
                        (
                            Rect { height, ..rect },
                            Rect {
                                y: rect.y + height,
                                height: rect.height - height,
                                ..rect
                            },
                            Rect {
                                y: rect.y + height,
                                height: 0.0,
                                ..rect
                            },
                        )
                    }
                };
                first.visit(a, panes, dividers);
                second.visit(b, panes, dividers);
                dividers.push(Divider {
                    id: *id,
                    axis: *axis,
                    rect: divider,
                    parent: rect,
                });
            }
        }
    }

    fn set_ratio(&mut self, id: usize, ratio: u16) -> bool {
        match self {
            Self::Split {
                id: node_id,
                ratio: current,
                ..
            } if *node_id == id => {
                *current = ratio.clamp(100, 900);
                true
            }
            Self::Split { first, second, .. } => {
                first.set_ratio(id, ratio) || second.set_ratio(id, ratio)
            }
            Self::Leaf(_) => false,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabGroups {
    placements: Vec<(TabId, GroupId)>,
    active: Vec<Option<TabId>>,
    focused_group: GroupId,
    root: Node,
    next_group: GroupId,
    next_split: usize,
}

impl Default for TabGroups {
    fn default() -> Self {
        Self {
            placements: Vec::new(),
            active: vec![None],
            focused_group: 0,
            root: Node::Leaf(0),
            next_group: 1,
            next_split: 0,
        }
    }
}

impl TabGroups {
    pub fn add(&mut self, id: TabId, group: GroupId) {
        let group = if self.root.has(group) {
            group
        } else {
            self.focused_group
        };
        self.placements.push((id, group));
        self.active[group] = Some(id);
        self.focused_group = group;
    }

    pub fn add_background(&mut self, id: TabId, group: GroupId) -> bool {
        let group = if self.root.has(group) {
            group
        } else {
            self.focused_group
        };
        let previous = self.active[group];
        let focused = self.focused_group;
        self.add(id, group);
        if let Some(previous) = previous {
            self.activate(previous);
        }
        self.set_focused_group(focused);
        self.active[group] == Some(id) && self.focused_group == group
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
        let group = self.placements.remove(index).1;
        self.replace_active(group, id);
        if self.group_ids(group).is_empty() {
            self.prune(group);
        }
        true
    }

    pub fn dock_into(&mut self, id: TabId, target: GroupId, dock: Dock) -> bool {
        let Some(source) = self.group_of(id) else {
            return false;
        };
        if !self.root.has(target) {
            return false;
        }
        if source == target && (dock == Dock::Center || self.group_ids(source).len() == 1) {
            return false;
        }
        let source_empties = self.group_ids(source).len() == 1;
        let destination = if dock == Dock::Center {
            target
        } else {
            let new_group = if source != target && source_empties {
                self.root.remove(source);
                source
            } else {
                let group = self.next_group;
                self.next_group += 1;
                self.active.resize(self.next_group, None);
                group
            };
            let split_id = self.next_split;
            self.next_split += 1;
            assert!(self.root.split(target, new_group, dock, split_id));
            new_group
        };
        self.placements
            .iter_mut()
            .find(|(tab_id, _)| *tab_id == id)
            .unwrap()
            .1 = destination;
        if source != destination {
            self.replace_active(source, id);
        }
        self.active[destination] = Some(id);
        self.focused_group = destination;
        if dock == Dock::Center && source_empties {
            self.prune(source);
        }
        true
    }

    pub fn cycle(&mut self, delta: i32) -> Option<TabId> {
        let ids = self.group_ids(self.focused_group);
        let index = self
            .active(self.focused_group)
            .and_then(|id| ids.iter().position(|x| *x == id));
        let next = cycle_index(index, ids.len(), delta).map(|index| ids[index]);
        self.active[self.focused_group] = next;
        next
    }

    pub fn set_focused_group(&mut self, group: GroupId) {
        if self.active(group).is_some() {
            self.focused_group = group;
        }
    }
    pub fn focused_group(&self) -> GroupId {
        self.focused_group
    }
    pub fn active(&self, group: GroupId) -> Option<TabId> {
        self.active.get(group).copied().flatten()
    }
    pub fn group_of(&self, id: TabId) -> Option<GroupId> {
        self.placements
            .iter()
            .find_map(|(tab_id, group)| (*tab_id == id).then_some(*group))
    }
    pub fn group_ids(&self, group: GroupId) -> Vec<TabId> {
        self.placements
            .iter()
            .filter_map(|(id, tab_group)| (*tab_group == group).then_some(*id))
            .collect()
    }
    pub fn groups(&self) -> Vec<GroupId> {
        self.layout()
            .0
            .into_iter()
            .map(|(group, _)| group)
            .collect()
    }
    pub fn layout(&self) -> (Vec<(GroupId, Rect)>, Vec<Divider>) {
        let mut panes = Vec::new();
        let mut dividers = Vec::new();
        self.root.visit(
            Rect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            &mut panes,
            &mut dividers,
        );
        (panes, dividers)
    }
    pub fn set_split_ratio(&mut self, id: usize, ratio: u16) -> bool {
        self.root.set_ratio(id, ratio)
    }
    fn replace_active(&mut self, group: GroupId, id: TabId) {
        if self.active[group] == Some(id) {
            self.active[group] = self
                .placements
                .iter()
                .rev()
                .find_map(|(tab_id, tab_group)| (*tab_group == group).then_some(*tab_id));
        }
    }
    fn prune(&mut self, group: GroupId) {
        self.root.remove(group);
        if self.placements.is_empty() {
            self.root = Node::Leaf(0);
            self.focused_group = 0;
            return;
        }
        if self.focused_group == group || self.active(self.focused_group).is_none() {
            self.focused_group = self
                .groups()
                .into_iter()
                .find(|candidate| self.active(*candidate).is_some())
                .unwrap_or(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeated_nested_splits_and_collapse() {
        let mut tabs = TabGroups::default();
        for id in 0..6 {
            tabs.add(id, 0);
        }
        assert!(tabs.dock_into(1, 0, Dock::Right));
        let right = tabs.group_of(1).unwrap();
        assert!(tabs.dock_into(2, right, Dock::Bottom));
        let bottom = tabs.group_of(2).unwrap();
        assert!(tabs.dock_into(3, bottom, Dock::Right));
        assert!(tabs.dock_into(4, 0, Dock::Bottom));
        assert!(tabs.dock_into(5, right, Dock::Bottom));
        assert_eq!(tabs.groups().len(), 6);
        assert_eq!(tabs.layout().1.len(), 5);
        let removed = tabs.group_of(3).unwrap();
        assert!(tabs.remove(3));
        assert!(!tabs.groups().contains(&removed));
    }

    #[test]
    fn single_tab_cannot_split_own_pane() {
        let mut tabs = TabGroups::default();
        tabs.add(1, 0);
        assert!(!tabs.dock_into(1, 0, Dock::Right));
        assert_eq!(tabs.groups(), vec![0]);
    }

    #[test]
    fn last_moved_tab_reuses_group_id() {
        let mut tabs = TabGroups::default();
        tabs.add(1, 0);
        tabs.add(2, 0);
        assert!(tabs.dock_into(2, 0, Dock::Right));
        assert!(tabs.dock_into(1, 1, Dock::Bottom));
        assert_eq!(tabs.groups().len(), 2);
        assert_eq!(tabs.group_of(1), Some(0));
    }

    #[test]
    fn center_drop_merges_and_preserves_other_splits() {
        let mut tabs = TabGroups::default();
        for id in 0..4 {
            tabs.add(id, 0);
        }
        assert!(tabs.dock_into(1, 0, Dock::Right));
        assert!(tabs.dock_into(2, 1, Dock::Bottom));
        assert_eq!(tabs.groups().len(), 3);
        assert!(tabs.dock_into(2, 0, Dock::Center));
        assert_eq!(tabs.groups().len(), 2);
        assert_eq!(tabs.group_of(2), Some(0));
        assert_eq!(tabs.active(0), Some(2));
        assert_eq!(tabs.focused_group(), 0);
        assert_eq!(tabs.group_of(1), Some(1));
    }

    #[test]
    fn divider_ratios_are_independent() {
        let mut tabs = TabGroups::default();
        for id in 0..3 {
            tabs.add(id, 0);
        }
        tabs.dock_into(1, 0, Dock::Right);
        tabs.dock_into(2, 1, Dock::Bottom);
        let (_, dividers) = tabs.layout();
        assert_eq!(dividers.len(), 2);
        let nested = dividers
            .iter()
            .find(|divider| divider.axis == Axis::Vertical)
            .unwrap();
        tabs.set_split_ratio(nested.id, 700);
        let (panes, dividers_after) = tabs.layout();
        assert_eq!(
            dividers_after
                .iter()
                .find(|divider| divider.axis == Axis::Horizontal)
                .unwrap()
                .rect
                .x,
            0.5
        );
        let right_top = panes.iter().find(|(group, _)| *group == 1).unwrap().1;
        assert!((right_top.height - 0.7).abs() < 0.001);
    }
}
