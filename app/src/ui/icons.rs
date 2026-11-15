//! Simple drawn icons, so the app needs no icon font.

use std::f32::consts::{PI, TAU};

use egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Mic,
    MicOff,
    Headphones,
    HeadphonesOff,
    Gear,
    Camera,
    CameraOff,
    Screen,
    HangUp,
    Hash,
    Speaker,
    Plus,
    Users,
    Expand,
    Close,
    Signal,
    Home,
    PopOut,
    Reply,
    More,
    Smile,
    Trash,
    Pin,
    PlusCircle,
    Phone,
    Send,
}

fn sw(rect: Rect) -> f32 {
    (rect.width() / 12.0).max(1.4)
}

fn slash(p: &Painter, rect: Rect, color: Color32) {
    let r = rect.shrink(rect.width() * 0.1);
    p.line_segment(
        [r.left_top(), r.right_bottom()],
        Stroke::new(sw(rect) * 1.6, Color32::from_black_alpha(160)),
    );
    p.line_segment(
        [r.left_top(), r.right_bottom()],
        Stroke::new(sw(rect), color),
    );
}

pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = Stroke::new(sw(rect), color);
    let c = rect.center();
    let u = rect.width() / 24.0;
    let at = |x: f32, y: f32| pos2(rect.left() + x * u, rect.top() + y * u);
    match icon {
        Icon::Mic | Icon::MicOff => {
            let body = Rect::from_min_max(at(9.0, 3.0), at(15.0, 14.0));
            p.rect_filled(body, 3.0 * u, color);
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = PI * i as f32 / 16.0;
                    pos2(c.x - 6.0 * u * a.cos(), at(0.0, 11.0).y + 6.0 * u * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, s));
            p.line_segment([at(12.0, 17.0), at(12.0, 21.0)], s);
            p.line_segment([at(8.5, 21.0), at(15.5, 21.0)], s);
            if icon == Icon::MicOff {
                slash(p, rect, Color32::from_rgb(0xed, 0x42, 0x45));
            }
        }
        Icon::Headphones | Icon::HeadphonesOff => {
            let pts: Vec<Pos2> = (0..=20)
                .map(|i| {
                    let a = PI + PI * i as f32 / 20.0;
                    pos2(c.x + 8.0 * u * a.cos(), at(0.0, 13.0).y + 8.0 * u * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, s));
            p.rect_filled(
                Rect::from_min_max(at(3.0, 13.0), at(8.0, 20.0)),
                1.5 * u,
                color,
            );
            p.rect_filled(
                Rect::from_min_max(at(16.0, 13.0), at(21.0, 20.0)),
                1.5 * u,
                color,
            );
            if icon == Icon::HeadphonesOff {
                slash(p, rect, Color32::from_rgb(0xed, 0x42, 0x45));
            }
        }
        Icon::Gear => {
            for i in 0..8 {
                let a = TAU * i as f32 / 8.0;
                let d = vec2(a.cos(), a.sin());
                p.line_segment(
                    [c + d * 5.0 * u, c + d * 9.5 * u],
                    Stroke::new(3.2 * u, color),
                );
            }
            p.circle_filled(c, 7.0 * u, color);
            p.circle_filled(c, 3.0 * u, Color32::from_rgb(0x29, 0x2b, 0x2f));
        }
        Icon::Camera | Icon::CameraOff => {
            p.rect_filled(
                Rect::from_min_max(at(2.0, 6.0), at(15.0, 18.0)),
                2.5 * u,
                color,
            );
            p.add(Shape::convex_polygon(
                vec![at(16.0, 12.0), at(22.0, 7.0), at(22.0, 17.0)],
                color,
                Stroke::NONE,
            ));
            if icon == Icon::CameraOff {
                slash(p, rect, Color32::from_rgb(0xed, 0x42, 0x45));
            }
        }
        Icon::Screen => {
            p.rect_stroke(
                Rect::from_min_max(at(2.5, 4.0), at(21.5, 16.5)),
                2.0 * u,
                s,
                egui::StrokeKind::Middle,
            );
            p.line_segment([at(12.0, 16.5), at(12.0, 20.0)], s);
            p.line_segment([at(7.0, 20.5), at(17.0, 20.5)], s);
            // Arrow up inside.
            p.line_segment([at(12.0, 13.0), at(12.0, 7.5)], s);
            p.line_segment([at(9.5, 10.0), at(12.0, 7.5)], s);
            p.line_segment([at(14.5, 10.0), at(12.0, 7.5)], s);
        }
        Icon::HangUp => {
            let pts: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = PI + PI * i as f32 / 16.0;
                    pos2(c.x + 9.0 * u * a.cos(), at(0.0, 15.0).y + 5.0 * u * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new(4.0 * u, color)));
            p.rect_filled(
                Rect::from_min_max(at(2.0, 13.5), at(7.0, 17.5)),
                1.5 * u,
                color,
            );
            p.rect_filled(
                Rect::from_min_max(at(17.0, 13.5), at(22.0, 17.5)),
                1.5 * u,
                color,
            );
        }
        Icon::Hash => {
            p.line_segment([at(9.5, 3.0), at(7.5, 21.0)], s);
            p.line_segment([at(16.5, 3.0), at(14.5, 21.0)], s);
            p.line_segment([at(4.0, 8.5), at(21.0, 8.5)], s);
            p.line_segment([at(3.0, 15.5), at(20.0, 15.5)], s);
        }
        Icon::Speaker => {
            p.add(Shape::convex_polygon(
                vec![
                    at(3.0, 9.0),
                    at(7.0, 9.0),
                    at(12.0, 4.0),
                    at(12.0, 20.0),
                    at(7.0, 15.0),
                    at(3.0, 15.0),
                ],
                color,
                Stroke::NONE,
            ));
            for (rad, n) in [(4.5f32, 10), (8.0f32, 14)] {
                let pts: Vec<Pos2> = (0..=n)
                    .map(|i| {
                        let a = -PI / 4.0 + (PI / 2.0) * i as f32 / n as f32;
                        pos2(at(12.0, 0.0).x + rad * u * a.cos(), c.y + rad * u * a.sin())
                    })
                    .collect();
                p.add(Shape::line(pts, s));
            }
        }
        Icon::Plus => {
            p.line_segment([at(12.0, 4.0), at(12.0, 20.0)], s);
            p.line_segment([at(4.0, 12.0), at(20.0, 12.0)], s);
        }
        Icon::Users => {
            p.circle_filled(at(9.0, 8.0), 3.5 * u, color);
            p.add(Shape::convex_polygon(
                vec![
                    at(2.0, 20.0),
                    at(3.5, 14.0),
                    at(9.0, 12.5),
                    at(14.5, 14.0),
                    at(16.0, 20.0),
                ],
                color,
                Stroke::NONE,
            ));
            p.circle_filled(at(16.5, 7.0), 3.0 * u, color);
            p.line_segment([at(17.0, 12.0), at(21.5, 13.5)], s);
            p.line_segment([at(21.5, 13.5), at(22.0, 18.0)], s);
        }
        Icon::Expand => {
            for (a, b, d) in [
                (at(4.0, 4.0), at(9.0, 4.0), at(4.0, 9.0)),
                (at(20.0, 4.0), at(15.0, 4.0), at(20.0, 9.0)),
                (at(4.0, 20.0), at(9.0, 20.0), at(4.0, 15.0)),
                (at(20.0, 20.0), at(15.0, 20.0), at(20.0, 15.0)),
            ] {
                p.line_segment([a, b], s);
                p.line_segment([a, d], s);
            }
        }
        Icon::Close => {
            p.line_segment([at(5.0, 5.0), at(19.0, 19.0)], s);
            p.line_segment([at(19.0, 5.0), at(5.0, 19.0)], s);
        }
        Icon::Signal => {
            for (i, h) in [6.0f32, 11.0, 16.0].iter().enumerate() {
                let x = 5.0 + i as f32 * 6.0;
                p.rect_filled(
                    Rect::from_min_max(at(x, 20.0 - h), at(x + 3.5, 20.0)),
                    1.0,
                    color,
                );
            }
        }
        Icon::Home => {
            p.add(Shape::convex_polygon(
                vec![at(12.0, 3.0), at(21.0, 11.0), at(3.0, 11.0)],
                color,
                Stroke::NONE,
            ));
            p.rect_filled(
                Rect::from_min_max(at(5.5, 10.5), at(18.5, 20.0)),
                1.0,
                color,
            );
        }
        Icon::PopOut => {
            p.rect_stroke(
                Rect::from_min_max(at(3.0, 7.0), at(17.0, 21.0)),
                2.0 * u,
                s,
                egui::StrokeKind::Middle,
            );
            p.line_segment([at(11.0, 13.0), at(21.0, 3.0)], s);
            p.line_segment([at(14.0, 3.0), at(21.0, 3.0)], s);
            p.line_segment([at(21.0, 3.0), at(21.0, 10.0)], s);
        }
        Icon::Reply => {
            p.line_segment([at(4.0, 11.0), at(10.0, 5.0)], s);
            p.line_segment([at(4.0, 11.0), at(10.0, 17.0)], s);
            p.line_segment([at(4.0, 11.0), at(14.0, 11.0)], s);
            p.line_segment([at(14.0, 11.0), at(20.0, 14.0)], s);
            p.line_segment([at(20.0, 14.0), at(20.0, 20.0)], s);
        }
        Icon::More => {
            for x in [5.0, 12.0, 19.0] {
                p.circle_filled(at(x, 12.0), 2.0 * u, color);
            }
        }
        Icon::Smile => {
            p.circle_stroke(c, 9.0 * u, s);
            p.circle_filled(at(9.0, 10.0), 1.3 * u, color);
            p.circle_filled(at(15.0, 10.0), 1.3 * u, color);
            let pts: Vec<Pos2> = (0..=12)
                .map(|i| {
                    let a = PI * i as f32 / 12.0;
                    pos2(c.x + 4.5 * u * a.cos(), at(0.0, 13.0).y + 3.5 * u * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, s));
        }
        Icon::Trash => {
            p.line_segment([at(4.0, 6.0), at(20.0, 6.0)], s);
            p.line_segment([at(9.0, 6.0), at(9.0, 3.5)], s);
            p.line_segment([at(15.0, 6.0), at(15.0, 3.5)], s);
            p.line_segment([at(9.0, 3.5), at(15.0, 3.5)], s);
            p.line_segment([at(6.0, 6.0), at(7.0, 21.0)], s);
            p.line_segment([at(18.0, 6.0), at(17.0, 21.0)], s);
            p.line_segment([at(7.0, 21.0), at(17.0, 21.0)], s);
        }
        Icon::Pin => {
            p.add(Shape::convex_polygon(
                vec![
                    at(8.0, 3.0),
                    at(16.0, 3.0),
                    at(15.0, 11.0),
                    at(18.5, 14.0),
                    at(5.5, 14.0),
                    at(9.0, 11.0),
                ],
                color,
                Stroke::NONE,
            ));
            p.line_segment([at(12.0, 14.0), at(12.0, 21.0)], s);
        }
        Icon::PlusCircle => {
            p.circle_filled(c, 10.0 * u, color);
            let dark = Color32::from_rgb(0x38, 0x3a, 0x40);
            let k = Stroke::new(sw(rect) * 1.2, dark);
            p.line_segment([at(12.0, 7.0), at(12.0, 17.0)], k);
            p.line_segment([at(7.0, 12.0), at(17.0, 12.0)], k);
        }
        Icon::Phone => {
            let pts: Vec<Pos2> = (0..=14)
                .map(|i| {
                    let a = PI * 0.15 + PI * 0.7 * i as f32 / 14.0;
                    pos2(
                        at(12.0, 0.0).x + 8.5 * u * a.cos(),
                        at(0.0, 12.0).y + 8.5 * u * a.sin(),
                    )
                })
                .collect();
            p.add(Shape::line(pts, Stroke::new(sw(rect) * 1.6, color)));
            p.rect_filled(
                Rect::from_min_max(at(15.0, 15.5), at(20.5, 20.0)),
                1.5 * u,
                color,
            );
            p.rect_filled(
                Rect::from_min_max(at(3.5, 4.0), at(9.0, 8.5)),
                1.5 * u,
                color,
            );
        }
        Icon::Send => {
            p.add(Shape::convex_polygon(
                vec![at(3.0, 4.0), at(21.0, 12.0), at(3.0, 20.0), at(6.0, 12.0)],
                color,
                Stroke::NONE,
            ));
        }
    }
}
