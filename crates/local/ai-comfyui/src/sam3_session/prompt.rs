//! The geometric prompt a person gives SAM 3: points and a box, in pixels of the prepared image.
/// Everything the installed SAM 3 model accepts in one geometric prompt.
pub mod sam3 {
    use serde_json::{Value, json};
    /// One geometric prompt: any mix of positive points, negative points and one box,
    /// all in pixels of the prepared image.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Prompt {
        pub positive_points: Vec<(f32, f32)>,
        pub negative_points: Vec<(f32, f32)>,
        pub box_: Option<[f32; 4]>,
    }

    impl Prompt {
        pub fn point(x: f32, y: f32) -> Self {
            Self {
                positive_points: vec![(x, y)],
                ..Default::default()
            }
        }

        pub fn is_empty(&self) -> bool {
            self.positive_points.is_empty()
                && self.negative_points.is_empty()
                && self.box_.is_none()
        }

        pub(crate) fn store(&self, session: u128) -> Value {
            let points = |list: &[(f32, f32)]| {
                list.iter()
                    .map(|(x, y)| json!({"x": x, "y": y}))
                    .collect::<Vec<_>>()
            };
            json!([{
                "positive_points": points(&self.positive_points),
                "negative_points": points(&self.negative_points),
                "positive_boxes": match self.box_ {
                    Some([x1, y1, x2, y2]) => json!([{"x1": x1, "y1": y1, "x2": x2, "y2": y2}]),
                    None => json!([]),
                },
                "session": session,
            }])
        }
    }
}
