use super::*;
macro_rules! normalized {
    ($name:ident,$input:tt,$expected:expr) => {
        #[test]
        fn $name() {
            assert_eq!(normalize(&json!($input)).unwrap(), $expected);
        }
    };
}
normalized!(touching_intervals_merge, [[0, 1], [1, 10]], vec![(0, 10)]);
normalized!(
    overlapping_intervals_merge,
    [[0, 5], [1, 10]],
    vec![(0, 10)]
);
normalized!(
    contained_interval_does_not_extend_hours,
    [[0, 60], [1, 10]],
    vec![(0, 60)]
);
normalized!(
    sunday_monday_intervals_merge,
    [[9600, 10140], [60, 180]],
    vec![(9600, 10260)]
);
normalized!(
    long_cross_week_merge_splits,
    [[9600, 10140], [30, 4500]],
    vec![(0, 4500), (9600, 10080)]
);
normalized!(
    touching_cross_week_long_merge_splits,
    [[9600, 10140], [60, 4500]],
    vec![(0, 4500), (9600, 10080)]
);
normalized!(
    unsorted_intervals_sort,
    [[4860, 5340], [540, 1020]],
    vec![(540, 1020), (4860, 5340)]
);
normalized!(
    interval_is_clamped_to_one_week,
    [[10, 11520]],
    vec![(10, 10090)]
);
normalized!(empty_schedule_is_empty, [], vec![]);
macro_rules! refused {
    ($name:ident,$input:tt) => {
        #[test]
        fn $name() {
            assert!(normalize(&json!($input)).is_err());
        }
    };
}
refused!(negative_start_is_refused, [[-1, 60]]);
refused!(start_after_week_is_refused, [[10081, 10100]]);
refused!(end_after_eighth_day_is_refused, [[0, 11521]]);
refused!(zero_duration_is_refused, [[60, 60]]);
refused!(reversed_interval_is_refused, [[120, 60]]);
refused!(boolean_minutes_are_refused, [[true, 60]]);
refused!(fractional_minutes_are_refused, [[0.5, 60]]);
refused!(string_minutes_are_refused, [["0", 60]]);
refused!(malformed_interval_is_refused, [[0, 60, 70]]);
#[test]
fn more_than_28_unmerged_intervals_is_refused() {
    let values: Vec<_> = (0..29).map(|n| json!([n * 3, n * 3 + 1])).collect();
    assert!(normalize(&json!(values)).is_err());
}
