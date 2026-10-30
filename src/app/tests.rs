use super::{
    FRect, ImageTextureCacheEntry, add_rect_to_bounds, default_language_selection, edit_key_code,
    estimated_texture_bytes, gal_animation_frame, image_texture_cache_evictions,
    inferred_mask_group, interpolation_group_key, is_toolbar_visible, menu_content_rows,
    menu_geometry, menu_geometry_with_width, menu_hit, menu_hit_rows, menu_scrollbar_geometry,
    picture_present_floor, should_interpolate_gal, toolbar_contains, toolbar_hit,
    toolbar_in_reveal_strip, update_toolbar_open, waits_for_picture,
};
use crate::interpolation::InterpolationMode;
use crate::lsb::{Prop, expr::Value};
use crate::scene::{Kind, Scene};
use std::collections::{HashMap, HashSet};

#[test]
fn scene_cut_waits_for_the_picture_not_the_map() {
    let floor = picture_present_floor([95, 100, 505, 1102]).unwrap();
    assert_eq!(floor, 95);
    assert!(waits_for_picture(95, floor));
    assert!(waits_for_picture(100, floor));
    assert!(!waits_for_picture(505, floor));
    assert!(!waits_for_picture(1102, floor));
}

#[test]
fn interpolation_is_limited_to_large_image_gals_and_toolbar_hit_targets() {
    assert!(should_interpolate_gal(
        InterpolationMode::Crossfade,
        Kind::Image,
        200_000.0,
        600_000.0
    ));
    assert!(!should_interpolate_gal(
        InterpolationMode::Crossfade,
        Kind::Image,
        199_999.0,
        600_000.0
    ));
    assert!(!should_interpolate_gal(
        InterpolationMode::Crossfade,
        Kind::Button,
        600_000.0,
        600_000.0
    ));
    assert!(!should_interpolate_gal(
        InterpolationMode::Off,
        Kind::Image,
        600_000.0,
        600_000.0
    ));
    // Los puntos se toman del centro de cada rect para seguir al layout real.
    let mid = |r: FRect| (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let (ox, oy) = mid(super::toolbar_button_rects(800)
        .into_iter()
        .find(|(a, _)| matches!(a, super::ToolbarAction::Options))
        .expect("botón Options")
        .1);
    assert!(matches!(
        toolbar_hit(800, ox, oy, false),
        Some(super::ToolbarAction::Options)
    ));
    let (ix, iy) = mid(super::interpolation_option_rects(800)[1].1);
    assert!(matches!(
        toolbar_hit(800, ix, iy, true),
        Some(super::ToolbarAction::Interpolation(
            InterpolationMode::Crossfade
        ))
    ));
    let (fx, fy) = mid(super::fps_meter_option_rect(800));
    assert!(matches!(
        toolbar_hit(800, fx, fy, true),
        Some(super::ToolbarAction::ToggleFpsMeter)
    ));
    let (ex, ey) = mid(super::export_texts_option_rect(800));
    assert!(matches!(
        toolbar_hit(800, ex, ey, true),
        Some(super::ToolbarAction::ExportTexts)
    ));
    let (sx, sy) = mid(super::scale_filter_option_rects(800)[0].1);
    assert!(matches!(
        toolbar_hit(800, sx, sy, true),
        Some(super::ToolbarAction::ScaleFilter(_))
    ));
    assert!(
        toolbar_hit(800, fx, fy, false).is_none(),
        "opciones cerradas: sin acciones del panel"
    );
    assert!(toolbar_in_reveal_strip(0.0));
    assert!(toolbar_in_reveal_strip(5.0));
    assert!(!toolbar_in_reveal_strip(6.0));
    assert!(!is_toolbar_visible(false, false));
    assert!(is_toolbar_visible(true, false));
    assert!(is_toolbar_visible(false, true));
    assert!(toolbar_contains(800, 400.0, 20.0, false));
    assert!(toolbar_contains(800, 400.0, 40.0, false));
    assert!(toolbar_contains(
        800,
        400.0,
        super::TOOLBAR_BAR_HEIGHT - 1.0,
        false
    ));
    assert!(!toolbar_contains(
        800,
        400.0,
        super::TOOLBAR_BAR_HEIGHT + 1.0,
        false
    ));
    assert!(toolbar_contains(800, 700.0, 100.0, true));
    let mut toolbar_open = false;
    let mut options_open = false;
    update_toolbar_open(&mut toolbar_open, &mut options_open, 800, (400.0, 3.0));
    assert!(toolbar_open);
    update_toolbar_open(&mut toolbar_open, &mut options_open, 800, (400.0, 20.0));
    assert!(toolbar_open);
    update_toolbar_open(&mut toolbar_open, &mut options_open, 800, (400.0, 80.0));
    assert!(!toolbar_open);
    assert!(!super::point_in_logical_render((-1.0, 300.0), (800, 600)));
    assert!(!super::point_in_logical_render((400.0, 600.0), (800, 600)));
    assert!(super::point_in_logical_render((799.0, 599.0), (800, 600)));
}

#[test]
fn language_picker_defaults_to_first_translation_when_available() {
    assert_eq!(default_language_selection(0), 0);
    assert_eq!(default_language_selection(1), 1);
    assert_eq!(default_language_selection(3), 1);
}

#[test]
fn image_texture_cache_evicts_lru_sources_but_protects_used_sources() {
    let cache = HashMap::from([
        (
            "old".to_string(),
            ImageTextureCacheEntry {
                bytes: 200 * 1024 * 1024,
                last_used: 1,
            },
        ),
        (
            "recent".to_string(),
            ImageTextureCacheEntry {
                bytes: 20 * 1024 * 1024,
                last_used: 2,
            },
        ),
        (
            "current".to_string(),
            ImageTextureCacheEntry {
                bytes: 20 * 1024 * 1024,
                last_used: 3,
            },
        ),
    ]);
    let used_this_frame = HashSet::from(["recent".to_string()]);

    assert_eq!(
        image_texture_cache_evictions(&cache, 100 * 1024 * 1024, "current", &used_this_frame),
        vec!["old"]
    );
}

#[test]
fn texture_memory_estimate_uses_rgba_bytes_and_saturates() {
    assert_eq!(estimated_texture_bytes(16, 8), 16 * 8 * 4);
    assert_eq!(estimated_texture_bytes(u32::MAX, u32::MAX), usize::MAX);
}

#[test]
fn crossfade_requires_opaque_frames_and_an_opaque_object() {
    let image = crate::gal::Image {
        w: 1,
        h: 1,
        rgba: vec![10, 20, 30, 255],
        frames: vec![vec![30, 20, 10, 255], vec![30, 20, 10, 0]],
        delays: vec![100, 100, 100],
    };

    assert!(super::can_crossfade_frames(&image, 0, 1, 255));
    assert!(!super::can_crossfade_frames(&image, 0, 2, 255));
    assert!(!super::can_crossfade_frames(&image, 0, 1, 128));
}

#[test]
fn interpolation_groups_use_explicit_tags_or_non_root_parents() {
    let mut scene = Scene::default();
    scene.create("container", Kind::Box, HashMap::new());
    scene.create(
        "base",
        Kind::Image,
        HashMap::from([
            (Prop::PARENT, Value::Str("container".into())),
            (Prop::TAG, Value::Int(7)),
        ]),
    );
    scene.create(
        "mask",
        Kind::Image,
        HashMap::from([
            (Prop::PARENT, Value::Str("container".into())),
            (Prop::TAG, Value::Int(7)),
        ]),
    );
    scene.create(
        "unrelated",
        Kind::Image,
        HashMap::from([
            (Prop::PARENT, Value::Str("container".into())),
            (Prop::TAG, Value::Int(8)),
        ]),
    );

    assert_eq!(
        interpolation_group_key(&scene, scene.get("base").unwrap()),
        interpolation_group_key(&scene, scene.get("mask").unwrap())
    );
    assert_ne!(
        interpolation_group_key(&scene, scene.get("base").unwrap()),
        interpolation_group_key(&scene, scene.get("unrelated").unwrap())
    );

    let mut untagged = Scene::default();
    untagged.create("container", Kind::Box, HashMap::new());
    untagged.create(
        "base",
        Kind::Image,
        HashMap::from([(Prop::PARENT, Value::Str("container".into()))]),
    );
    untagged.create(
        "mask",
        Kind::Image,
        HashMap::from([(Prop::PARENT, Value::Str("container".into()))]),
    );
    untagged.create(
        "unrelated",
        Kind::Image,
        HashMap::from([(Prop::PARENT, Value::Str(crate::vm::ROOT.into()))]),
    );
    assert_eq!(
        interpolation_group_key(&untagged, untagged.get("base").unwrap()),
        interpolation_group_key(&untagged, untagged.get("mask").unwrap())
    );
    assert_ne!(
        interpolation_group_key(&untagged, untagged.get("unrelated").unwrap()),
        interpolation_group_key(&untagged, untagged.get("base").unwrap())
    );
}

#[test]
fn interpolation_group_bounds_cover_related_sprite_union() {
    let mut bounds = None;
    add_rect_to_bounds(&mut bounds, (20.0, 30.0, 120.0, 130.0));
    add_rect_to_bounds(&mut bounds, (90.0, 60.0, 150.0, 90.0));

    let (left, top, right, bottom) = bounds.unwrap();
    assert_eq!((right - left) * (bottom - top), 13_000.0);
}

#[test]
fn inferred_mask_groups_require_matching_animation_timing_and_overlap() {
    let delays = [67, 67, 133];
    assert!(inferred_mask_group(
        (0.0, 0.0, 800.0, 600.0),
        &delays,
        (300.0, 200.0, 500.0, 400.0),
        &delays
    ));
    assert!(!inferred_mask_group(
        (0.0, 0.0, 800.0, 600.0),
        &delays,
        (300.0, 200.0, 500.0, 400.0),
        &[67, 50, 133]
    ));
    assert!(!inferred_mask_group(
        (0.0, 0.0, 800.0, 600.0),
        &delays,
        (790.0, 590.0, 890.0, 690.0),
        &delays
    ));
}

#[test]
fn gal_menu_frames_loop_by_their_declared_delays() {
    let delays = [100, 200, 300];
    assert_eq!(gal_animation_frame(&delays, 0.0), Some(0));
    assert_eq!(gal_animation_frame(&delays, 100.0), Some(1));
    assert_eq!(gal_animation_frame(&delays, 299.0), Some(1));
    assert_eq!(gal_animation_frame(&delays, 300.0), Some(2));
    assert_eq!(gal_animation_frame(&delays, 600.0), Some(0));
}

#[test]
fn edit_keys_use_original_script_virtual_key_codes() {
    assert_eq!(edit_key_code("Return"), Some(13));
    assert_eq!(edit_key_code("Escape"), Some(27));
    assert_eq!(edit_key_code("Tab"), Some(9));
    assert_eq!(edit_key_code("a"), Some(65));
    assert_eq!(edit_key_code("F1"), None);
}

#[test]
fn native_menu_hit_testing_uses_script_position() {
    let choices = vec!["first".to_string(), "second".to_string()];
    let mut scene = Scene::default();
    scene.create(
        "menu",
        Kind::Menu,
        HashMap::from([
            (Prop::LEFT, Value::Int(20)),
            (Prop::TOP, Value::Int(30)),
            (Prop::WIDTH, Value::Int(200)),
        ]),
    );
    let menu = scene.get("menu");

    assert_eq!(menu_hit(&choices, menu, 800, 600, 40.0, 40.0), Some(0));
    assert_eq!(menu_hit(&choices, menu, 800, 600, 400.0, 300.0), None);
}

#[test]
fn native_menu_hit_testing_maps_visible_rows_to_full_choice_indices() {
    let choices: Vec<_> = (0..30).map(|index| format!("slot {index}")).collect();
    let mut scene = Scene::default();
    scene.create(
        "menu",
        Kind::Menu,
        HashMap::from([
            (Prop::LEFT, Value::Int(20)),
            (Prop::TOP, Value::Int(30)),
            (Prop::WIDTH, Value::Int(200)),
            (Prop::FONTHEIGHT, Value::Int(20)),
            (Prop::ROWCOUNT, Value::Int(10)),
            (Prop::TOPINDEX, Value::Int(12)),
        ]),
    );
    let menu = scene.get("menu");

    assert_eq!(menu_hit(&choices, menu, 800, 600, 40.0, 31.0), Some(12));
    assert_eq!(menu_hit(&choices, menu, 800, 600, 40.0, 230.0), None);
}

#[test]
fn native_menu_uses_measured_width_unless_script_sets_width() {
    let choices = vec!["short".to_string(), "a much longer option".to_string()];
    let mut scene = Scene::default();
    scene.create(
        "menu",
        Kind::Menu,
        HashMap::from([(Prop::TOP, Value::Int(30))]),
    );

    let (frame, _) = menu_geometry_with_width(&choices, scene.get("menu"), 800, 600, Some(240.0));
    assert_eq!(frame.w, 240.0);
    assert_eq!(frame.x, 280.0);

    scene.set("menu", Prop::WIDTH, Value::Int(0));
    let (frame, _) = menu_geometry_with_width(&choices, scene.get("menu"), 800, 600, Some(240.0));
    assert_eq!(frame.w, 240.0);
    assert_eq!(frame.x, 280.0);

    scene.set("menu", Prop::ROWCOUNT, Value::Int(10));
    scene.set("menu", Prop::LEFT, Value::Int(400));
    let (frame, _) = menu_geometry_with_width(&choices, scene.get("menu"), 800, 600, Some(240.0));
    assert_eq!(frame.w, 240.0);
    assert_eq!(frame.x, 400.0);

    scene.set("menu", Prop::WIDTH, Value::Int(0));
    let (frame, _) = menu_geometry_with_width(&choices, scene.get("menu"), 800, 600, Some(240.0));
    assert_eq!(frame.w, 240.0);
    assert_eq!(frame.x, 400.0);

    scene.set("menu", Prop::WIDTH, Value::Int(200));
    let (frame, _) = menu_geometry_with_width(&choices, scene.get("menu"), 800, 600, Some(240.0));
    assert_eq!(frame.w, 200.0);
    assert_eq!(frame.x, 400.0);
}

#[test]
fn menu_scrollbar_is_vertical_on_the_right_and_tracks_top_index() {
    let frame = FRect::new(100.0, 100.0, 480.0, 300.0);
    let top = menu_scrollbar_geometry(frame, 120.0, 380.0, 8.0, (20, 20), 30, 10, 0)
        .expect("scrollbar should be visible when choices overflow");
    assert_eq!(top.x, 552.0);
    assert_eq!(top.top, 120.0);
    assert_eq!(top.button_size, 20.0);
    assert_eq!(top.track_height, 220.0);
    assert_eq!(top.thumb_offset, 0.0);
    assert!((top.thumb_height - 220.0 / 3.0).abs() < f32::EPSILON);

    let bottom = menu_scrollbar_geometry(frame, 120.0, 380.0, 8.0, (20, 20), 30, 10, 20)
        .expect("scrollbar should be visible at the last page");
    assert_eq!(bottom.x, 552.0);
    assert!(
        (bottom.thumb_offset - (bottom.track_height - bottom.thumb_height)).abs() < f32::EPSILON
    );
    assert!(menu_scrollbar_geometry(frame, 120.0, 380.0, 8.0, (20, 20), 10, 10, 0).is_none());
}

#[test]
fn native_menu_rows_follow_script_font_and_spacing_properties() {
    let choices = vec!["first".to_string(), "second".to_string()];
    let mut scene = Scene::default();
    scene.create(
        "menu",
        Kind::Menu,
        HashMap::from([
            (Prop::LEFT, Value::Int(20)),
            (Prop::TOP, Value::Int(30)),
            (Prop::FONTHEIGHT, Value::Int(22)),
            (Prop::LINESPACE, Value::Int(4)),
            (Prop::MARGINY, Value::Int(2)),
        ]),
    );

    let (frame, rows) = menu_geometry(&choices, scene.get("menu"), 800, 600);

    assert_eq!(frame.h, 56.0);
    assert_eq!(rows[0].y, 32.0);
    assert_eq!(rows[0].h, 26.0);
    assert_eq!(rows[1].y, 58.0);
}

#[test]
fn native_menu_border_insets_reflow_all_rows_without_hiding_options() {
    let choices = vec![
        "first".to_string(),
        "second".to_string(),
        "third".to_string(),
    ];
    let mut scene = Scene::default();
    scene.create(
        "menu",
        Kind::Menu,
        HashMap::from([
            (Prop::LEFT, Value::Int(20)),
            (Prop::TOP, Value::Int(30)),
            (Prop::FONTHEIGHT, Value::Int(22)),
            (Prop::LINESPACE, Value::Int(4)),
            (Prop::MARGINY, Value::Int(0)),
        ]),
    );

    let (frame, rows) = menu_geometry(&choices, scene.get("menu"), 800, 600);
    let (frame, content_rows) = menu_content_rows(frame, &rows, 3.0, 4.0, 5.0, 7.0, 22.0);

    assert_eq!(content_rows.len(), choices.len());
    assert_eq!(content_rows[0].y, frame.y + 5.0);
    assert_eq!(content_rows[0].h, 22.0);
    assert_eq!(content_rows[1].h, content_rows[0].h);
    assert_eq!(content_rows[2].h, content_rows[0].h);
    assert_eq!(content_rows[0].x, frame.x + 3.0);
    assert_eq!(content_rows[0].w, frame.w - 7.0);
    assert!(content_rows[0].y >= frame.y);
    assert!(content_rows[2].y + content_rows[2].h <= frame.y + frame.h - 7.0);
    assert_eq!(
        menu_hit_rows(&content_rows, 30.0, content_rows[0].y + 1.0),
        Some(0)
    );
    assert_eq!(
        menu_hit_rows(&content_rows, 30.0, content_rows[1].y + 1.0),
        Some(1)
    );
    assert_eq!(
        menu_hit_rows(&content_rows, 30.0, content_rows[2].y + 1.0),
        Some(2)
    );
}

#[test]
fn native_menu_with_short_suboptions_expands_without_clipping_rows() {
    let choices = vec!["first".to_string(), "second".to_string()];
    let mut scene = Scene::default();
    scene.create(
        "menu",
        Kind::Menu,
        HashMap::from([
            (Prop::LEFT, Value::Int(20)),
            (Prop::TOP, Value::Int(30)),
            (Prop::FONTHEIGHT, Value::Int(22)),
            (Prop::LINESPACE, Value::Int(4)),
            (Prop::MARGINY, Value::Int(0)),
        ]),
    );

    let (frame, rows) = menu_geometry(&choices, scene.get("menu"), 800, 600);
    let (frame, content_rows) = menu_content_rows(frame, &rows, 10.0, 10.0, 30.0, 30.0, 22.0);

    assert_eq!(content_rows.len(), choices.len());
    assert_eq!(frame.h, 104.0);
    assert_eq!(content_rows[0].h, 22.0);
    assert_eq!(content_rows[1].h, 22.0);
    assert!(content_rows[0].y >= frame.y + 30.0);
    assert!(content_rows[1].y + content_rows[1].h <= frame.y + frame.h - 30.0);
    assert_eq!(
        menu_hit_rows(&content_rows, 30.0, content_rows[0].y + 1.0),
        Some(0)
    );
    assert_eq!(
        menu_hit_rows(&content_rows, 30.0, content_rows[1].y + 1.0),
        Some(1)
    );
}
