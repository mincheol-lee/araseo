pub fn tab_cycle_delta(control: bool, shift: bool, alt: bool, tab: bool) -> Option<i32> {
    (control && !alt && tab).then_some(if shift { -1 } else { 1 })
}
