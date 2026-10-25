//! Git sidebar data, rebuilt only when a status snapshot changes.
use crate::tree::GitStatus;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
};

#[derive(Debug)]
pub struct Change {
    pub path: PathBuf,
    pub name: String,
    pub directory: String,
    pub status: GitStatus,
}
#[derive(Debug)]
pub struct Group {
    pub root: PathBuf,
    pub name: String,
    pub changes: Vec<Change>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Row {
    Group(usize),
    Change { group: usize, change: usize },
}

pub fn groups(
    statuses: &HashMap<PathBuf, GitStatus>,
    repositories: &HashSet<PathBuf>,
    workspaces: &[PathBuf],
) -> Vec<Group> {
    let mut grouped = BTreeMap::<PathBuf, Vec<Change>>::new();
    for (path, status) in statuses {
        if *status == GitStatus::Clean {
            continue;
        }
        // A nested repository owns its own files, even when a parent is also
        // monitored. No filesystem calls or repository scans happen here.
        let root = path
            .ancestors()
            .find(|parent| repositories.contains(*parent))
            .or_else(|| {
                workspaces
                    .iter()
                    .filter(|root| path.starts_with(root))
                    .max_by_key(|root| root.components().count())
                    .map(PathBuf::as_path)
            })
            .or_else(|| path.parent());
        let Some(root) = root else {
            continue;
        };
        let relative = path.strip_prefix(root).unwrap_or(path);
        let directory = relative
            .parent()
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .unwrap_or_default();
        grouped.entry(root.to_path_buf()).or_default().push(Change {
            path: path.clone(),
            name: path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned(),
            directory,
            status: *status,
        });
    }
    let mut groups = grouped
        .into_iter()
        .map(|(root, mut changes)| {
            changes.sort_by_cached_key(|change| {
                (
                    change.path.to_string_lossy().to_lowercase(),
                    change.path.clone(),
                )
            });
            Group {
                name: root
                    .file_name()
                    .unwrap_or(root.as_os_str())
                    .to_string_lossy()
                    .into_owned(),
                root,
                changes,
            }
        })
        .collect::<Vec<_>>();
    groups.sort_by_cached_key(|group| {
        (
            group.root.to_string_lossy().to_lowercase(),
            group.root.clone(),
        )
    });
    groups
}
/// Only explicitly expanded projects expose their changes; new projects start closed.
pub fn rows(groups: &[Group], expanded: &HashSet<PathBuf>) -> Vec<Row> {
    let mut rows = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        rows.push(Row::Group(index));
        if expanded.contains(&group.root) {
            rows.extend((0..group.changes.len()).map(|change| Row::Change {
                group: index,
                change,
            }));
        }
    }
    rows
}
pub fn status_label(status: GitStatus) -> &'static str {
    match status {
        GitStatus::Modified => "M",
        GitStatus::Added => "A",
        GitStatus::Untracked => "U",
        GitStatus::Deleted => "D",
        GitStatus::Clean => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn groups_by_deepest_repository_and_preserves_relative_directories() {
        let repos = HashSet::from([
            PathBuf::from("/work/project"),
            "/work/project/nested".into(),
            "/work/other".into(),
        ]);
        let statuses = HashMap::from([
            (
                PathBuf::from("/work/project/src/main.rs"),
                GitStatus::Modified,
            ),
            (
                "/work/project/nested/src/main.rs".into(),
                GitStatus::Untracked,
            ),
            ("/work/other/main.rs".into(), GitStatus::Deleted),
            ("/work/project/clean.rs".into(), GitStatus::Clean),
        ]);
        let groups = groups(&statuses, &repos, &["/work".into()]);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups.iter().map(|g| g.changes.len()).sum::<usize>(), 3);
        let nested = groups.iter().find(|g| g.name == "nested").unwrap();
        assert_eq!(nested.changes[0].directory, "src");
        assert_eq!(nested.changes[0].name, "main.rs");
        assert_eq!(status_label(nested.changes[0].status), "U");
        let expanded = groups
            .iter()
            .filter(|group| group.root != nested.root)
            .map(|group| group.root.clone())
            .collect();
        let rows = rows(&groups, &expanded);
        assert_eq!(rows.len(), 5);
        let nested_index = groups.iter().position(|g| g.name == "nested").unwrap();
        assert!(rows.contains(&Row::Group(nested_index)));
        assert!(!rows.contains(&Row::Change {
            group: nested_index,
            change: 0
        }));
    }
    #[test]
    fn projects_start_closed_and_status_refresh_preserves_explicit_expansion() {
        let alpha = PathBuf::from("/work/alpha");
        let beta = PathBuf::from("/work/beta");
        let repositories = HashSet::from([alpha.clone(), beta.clone()]);
        let mut statuses = HashMap::from([(alpha.join("main.rs"), GitStatus::Modified)]);
        let mut expanded = HashSet::new();
        let initial = groups(&statuses, &repositories, &[]);
        assert_eq!(rows(&initial, &expanded), vec![Row::Group(0)]);
        expanded.insert(alpha.clone());
        assert_eq!(
            rows(&initial, &expanded),
            vec![
                Row::Group(0),
                Row::Change {
                    group: 0,
                    change: 0
                }
            ]
        );
        statuses.insert(alpha.join("new.rs"), GitStatus::Added);
        statuses.insert(beta.join("main.rs"), GitStatus::Modified);
        let refreshed = groups(&statuses, &repositories, &[]);
        assert_eq!(
            rows(&refreshed, &expanded),
            vec![
                Row::Group(0),
                Row::Change {
                    group: 0,
                    change: 0
                },
                Row::Change {
                    group: 0,
                    change: 1
                },
                Row::Group(1)
            ]
        );
        expanded.remove(&alpha);
        assert_eq!(
            rows(&refreshed, &expanded),
            vec![Row::Group(0), Row::Group(1)]
        );
    }
    #[test]
    fn fallback_workspace_grouping_and_order_do_not_depend_on_hash_iteration() {
        let entries = [
            (PathBuf::from("/work/added/z.txt"), GitStatus::Added),
            (PathBuf::from("/work/added/A.txt"), GitStatus::Modified),
            (PathBuf::from("/work/root.txt"), GitStatus::Modified),
        ];
        let roots = ["/work".into(), "/work/added".into()];
        let forward = groups(
            &entries.clone().into_iter().collect(),
            &HashSet::new(),
            &roots,
        );
        let reverse = groups(
            &entries.into_iter().rev().collect(),
            &HashSet::new(),
            &roots,
        );
        let paths = |groups: &[Group]| {
            groups
                .iter()
                .flat_map(|g| g.changes.iter().map(|c| c.path.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(paths(&forward), paths(&reverse));
        assert_eq!(forward[1].root, PathBuf::from("/work/added"));
        assert_eq!(forward[1].changes[0].name, "A.txt");
        assert_eq!(forward[1].changes[0].directory, "");
    }
}
