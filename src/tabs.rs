pub fn cycle_index(active: Option<usize>, tab_count: usize, delta: i32) -> Option<usize> {
    if tab_count == 0 {
        return None;
    }

    let active = active.filter(|index| *index < tab_count).unwrap_or(0);
    Some((active as i64 + i64::from(delta)).rem_euclid(tab_count as i64) as usize)
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
}
