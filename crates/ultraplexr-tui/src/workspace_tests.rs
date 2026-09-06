use super::*;
#[test]
fn split_geometry_reserves_dividers_and_status_rows() {
    let vertical = split_rects(Some(Axis::Vertical), (80, 25), 0);
    assert_eq!(
        vertical[0].1,
        Rect {
            x: 0,
            y: 0,
            width: 39,
            height: 25
        }
    );
    assert_eq!(
        vertical[1].1,
        Rect {
            x: 40,
            y: 0,
            width: 40,
            height: 25
        }
    );
    let horizontal = split_rects(Some(Axis::Horizontal), (80, 25), 1);
    assert_eq!(
        horizontal[0].1,
        Rect {
            x: 0,
            y: 0,
            width: 80,
            height: 12
        }
    );
    assert_eq!(
        horizontal[1].1,
        Rect {
            x: 0,
            y: 13,
            width: 80,
            height: 12
        }
    );
    assert_eq!(
        split_rects(Some(Axis::Vertical), (10, 5), 1),
        vec![(
            1,
            Rect {
                x: 0,
                y: 0,
                width: 10,
                height: 5
            }
        )]
    );
}
