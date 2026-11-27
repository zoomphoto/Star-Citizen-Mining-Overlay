pub mod breadcrumbs;
pub mod config;
pub mod detection;
pub mod lookup;
pub mod mining;
pub mod models;
pub mod overlay;
pub mod pipeline;
pub mod waypoints;

#[cfg(test)]
mod tests {
    use crate::config::{
        AppSettings, AppSettingsToml, AttributeKind, AttributeTarget, ColorResolver, DossierNotes,
        ExclusionList, HotkeyText, PlayerEntry, SocialConfig, SocialConfigToml,
    };
    use crate::detection::{LetterModel, LookAlikes, O0EvidenceTable};
    use crate::lookup::{
        CacheDatabase, NotFoundKind, OrgMembersParser, OrgPageParser, PlayerOrgs,
        ProfilePageParser, RsiResult,
    };
    use crate::mining::{DigitTemplates, MiningTable};
    use crate::models::{BracketItem, MatchKind, OrgMembership, OverlayColor, PixelRect};
    use crate::overlay::{
        BracketMetrics, BracketRasterizer, SoftwareOverlayPainter, TestOverlayBuilder,
        TestOverlayPart, TrackingBoundary,
    };
    use crate::pipeline::{Hungarian, TimedOverlayPress, TimedOverlayRun};
    use chrono::{Duration, Utc};

    #[test]
    fn embedded_letter_model_loads_and_scores() {
        let model = LetterModel::embedded();
        assert!(model.charset().len() >= 36);
        let blank = vec![0.0f64; model.input_size()];
        let mut log_probs = vec![0.0f64; model.class_count()];
        model.predict_log_probs(&blank, &mut log_probs);
        assert!(log_probs.iter().all(|v| v.is_finite()));
    }

    #[test]
    fn settings_and_social_config_assets_parse_cleanly() {
        let settings_txt = include_str!("../../../assets/settings.txt");
        let loaded = AppSettingsToml::read(settings_txt);
        assert!(loaded.is_valid(), "errors: {:?}", loaded.errors);
        let round_trip = AppSettingsToml::write(&loaded.settings);
        let reloaded = AppSettingsToml::read(&round_trip);
        assert_eq!(
            loaded.settings.hotkey_toggle_overlay,
            reloaded.settings.hotkey_toggle_overlay
        );
        assert_eq!(
            loaded.settings.hotkey_timed_overlay,
            reloaded.settings.hotkey_timed_overlay
        );

        let social_txt = include_str!("../../../assets/social-config.txt");
        let social_loaded = SocialConfigToml::read(social_txt);
        assert!(
            social_loaded.is_valid(),
            "errors: {:?}",
            social_loaded.errors
        );
    }

    #[test]
    fn hotkey_parse_and_format() {
        let chord = HotkeyText::parse("Shift+F12").expect("Shift+F12 valid");
        assert!(chord.shift);
        assert_eq!(chord.format(), "Shift+F12");

        let interactive = HotkeyText::parse("Shift+0").expect("Shift+0 valid");
        assert!(interactive.shift);
        assert_eq!(interactive.format(), "Shift+0");
    }

    #[test]
    fn color_resolver_priority_and_ranks() {
        let mut cfg = SocialConfig::default();
        cfg.players.push(PlayerEntry {
            handle: "EnemyAce".to_string(),
            color: OverlayColor::Red,
        });
        cfg.upsert_org("ALLY", OverlayColor::Green);
        cfg.upsert_attribute_rule(
            &AttributeTarget::new(AttributeKind::Activity, Some("Piracy".to_string())),
            OverlayColor::Orange,
        );

        let mut pirate_org = OrgMembership::new(Some("PIR".to_string()), true);
        pirate_org.activities = vec!["Piracy".to_string()];

        let mut ally_org = OrgMembership::new(Some("ALLY".to_string()), false);
        ally_org.activities = vec!["Security".to_string()];

        // Tier 1: Player list beats Org list & Org-Attribute
        let m1 = ColorResolver::resolve(&cfg, "EnemyAce", &[ally_org.clone(), pirate_org.clone()]);
        assert_eq!(m1.kind, MatchKind::PlayerList);
        assert_eq!(m1.color, OverlayColor::Red);

        // Tier 2: Org list beats Org-Attribute
        let m2 = ColorResolver::resolve(&cfg, "Wingman", &[pirate_org.clone(), ally_org]);
        assert_eq!(m2.kind, MatchKind::OrgList);
        assert_eq!(m2.color, OverlayColor::Green);
        assert_eq!(m2.deciding_org_sid.as_deref(), Some("ALLY"));

        // Tier 3: Org-Attribute matches Piracy
        let m3 = ColorResolver::resolve(&cfg, "Stranger", &[pirate_org]);
        assert_eq!(m3.kind, MatchKind::OrgAttribute);
        assert_eq!(m3.color, OverlayColor::Orange);
    }

    #[test]
    fn dossier_notes_rename_linking_and_sc_year() {
        let notes = DossierNotes::new(DossierNotes::DEFAULT_MAX_FILE_BYTES);
        assert!(notes.set(
            "OldHandle",
            Some("123456"),
            "Friendly medic at GrimHEX",
            Utc::now()
        ));
        assert_eq!(
            notes.panel_text("OldHandle").as_deref(),
            Some("Friendly medic at GrimHEX")
        );

        // Simulate handle rename -> NewHandle via Citizen Record 123456
        let (linked, changed) = notes.link("NewHandle", Some("123456"));
        assert!(changed);
        let updated_note = linked.expect("linked note");
        assert!(updated_note.text.contains("Friendly medic at GrimHEX"));
        assert!(
            updated_note.text.contains("295"),
            "expected +930 SC year in {}",
            updated_note.text
        );
    }

    #[test]
    fn sqlite_cache_v3_roundtrip_and_not_found_ttl() {
        let db = CacheDatabase::open_in_memory().expect("in-memory sqlite");
        assert_eq!(db.schema_version().unwrap(), 3);

        let now = Utc::now();
        let mut org = OrgMembership::new(Some("TESTORG".to_string()), true);
        org.archetype = Some("Organization".to_string());
        org.activities = vec!["Trading".to_string(), "Exploration".to_string()];
        org.name = Some("Test Organization".to_string());

        let po = PlayerOrgs {
            handle: "CitizenOne".to_string(),
            orgs: vec![org],
        };
        db.save_player(&po, "test", now).unwrap();

        let fetched = db
            .get_player("citizenone")
            .expect("case-insensitive lookup");
        assert_eq!(fetched.handle, "CitizenOne");
        assert_eq!(fetched.orgs.len(), 1);
        assert_eq!(fetched.orgs[0].sid.as_deref(), Some("TESTORG"));
        assert_eq!(fetched.orgs[0].activities.len(), 2);

        // Escalating NotFound TTL: 1st miss -> 24h, 2nd miss -> 7d
        db.record_not_found(NotFoundKind::Player, "GhostHandle", "test", now)
            .unwrap();
        let nf1 = db
            .get_not_found(NotFoundKind::Player, "ghosthandle")
            .unwrap();
        assert_eq!(nf1.miss_count, 1);
        assert!(nf1.is_fresh(now + Duration::hours(12)));
        assert!(!nf1.is_fresh(now + Duration::hours(25)));

        db.record_not_found(NotFoundKind::Player, "GhostHandle", "test", now)
            .unwrap();
        let nf2 = db
            .get_not_found(NotFoundKind::Player, "GhostHandle")
            .unwrap();
        assert_eq!(nf2.miss_count, 2);
        assert!(nf2.is_fresh(now + Duration::days(5)));
    }

    #[test]
    fn rsi_parsers_handle_not_found_and_valid_markup() {
        let broken_html = r#"<html><body><div>Unexpected layout</div></body></html>"#;
        assert!(matches!(
            ProfilePageParser::parse(broken_html, "Nobody"),
            RsiResult::Failed(_)
        ));

        let profile_html = r#"
            <html><body>
                <div class="profile">
                    <p class="citizen-record"><strong class="value">#987654</strong></p>
                    <div class="info">
                        <p class="entry"><strong class="value">Nitebird</strong></p>
                        <p class="entry"><span class="label">Handle name</span><strong class="value">Niteb1rd</strong></p>
                    </div>
                </div>
            </body></html>
        "#;
        if let RsiResult::Found(prof) = ProfilePageParser::parse(profile_html, "Niteb1rd") {
            assert_eq!(prof.handle, "Niteb1rd");
            assert_eq!(prof.citizen_record.as_deref(), Some("987654"));
            assert_eq!(prof.moniker.as_deref(), Some("Nitebird"));
        } else {
            panic!("expected ProfilePageParser::parse Found");
        }

        let org_html = r#"
            <html><body>
                <div id="organization">
                    <h1>Vanguard Wing / <span class="symbol">VANG</span></h1>
                    <span class="count">42 members</span>
                    <ul class="tags">
                        <li class="model">Corporation</li>
                        <li class="commitment">Hardcore</li>
                    </ul>
                    <ul class="focus">
                        <li class="primary"><span>Security</span></li>
                        <li class="secondary"><span>Bounty Hunting</span></li>
                    </ul>
                </div>
            </body></html>
        "#;
        if let RsiResult::Found(info) = OrgPageParser::parse(org_html, "vang") {
            assert_eq!(info.sid, "VANG");
            assert_eq!(info.member_count, Some(42));
            assert_eq!(info.archetype.as_deref(), Some("Corporation"));
        } else {
            panic!("expected OrgPageParser::parse Found");
        }

        let members_json = r#"{
            "success": 1,
            "data": {
                "totalrows": 2,
                "html": "<li class=\"member-item\"><a href=\"/citizens/PilotAlpha\"><span class=\"nick\">PilotAlpha</span></a></li><li class=\"member-item\"><a href=\"/citizens/PilotBeta\"><span class=\"nick\">PilotBeta</span></a></li>"
            }
        }"#;
        if let RsiResult::Found(page) = OrgMembersParser::parse_response(members_json) {
            assert_eq!(page.total_rows, Some(2));
            assert_eq!(page.handles, vec!["PilotAlpha", "PilotBeta"]);
        } else {
            panic!("expected OrgMembersParser::parse_response Found");
        }
    }

    #[test]
    fn mining_table_and_digit_templates_load() {
        let table = MiningTable::embedded();
        let quant = table.lookup(3170).unwrap_or("");
        assert!(
            quant.to_ascii_lowercase().contains("quant"),
            "expected Quantainium at 3170, got {quant}"
        );
        let double_quant = table.lookup(6340).unwrap_or("");
        assert!(
            double_quant.contains("2x"),
            "expected 2x cluster at 6340, got {double_quant}"
        );

        let pin = DigitTemplates::pin_template();
        assert_eq!(
            pin.len(),
            DigitTemplates::PIN_WIDTH * DigitTemplates::PIN_HEIGHT
        );
        let blank_patch = vec![0.0f32; DigitTemplates::WIDTH * DigitTemplates::HEIGHT];
        let (digit, _score, _all) = DigitTemplates::match_patch(&blank_patch, 0.08, false);
        assert!(digit.is_ascii_digit());
    }

    #[test]
    fn hungarian_solver_and_lookalikes() {
        let cost = [
            10.0, 1.0, 9.0, 9.0, // row 0 -> col 1 (1.0)
            9.0, 9.0, 2.0, 8.0, // row 1 -> col 2 (2.0)
            3.0, 9.0, 9.0, 7.0, // row 2 -> col 0 (3.0)
        ];
        let assigned = Hungarian::solve(&cost, 3, 4).expect("valid assignment");
        assert_eq!(assigned, vec![1, 2, 0]);

        assert_eq!(
            LookAlikes::fold_o0("N0M4D_01"),
            LookAlikes::fold_o0("NOM4D_O1")
        );
        let mut o0 = O0EvidenceTable::default();
        assert_eq!(o0.resolve("PIL0T", &[]), "PIL0T");
    }

    #[test]
    fn timed_overlay_and_software_painter_render_bgra8() {
        let mut timed = TimedOverlayRun::default();
        assert_eq!(timed.press(true, 10, 1000), TimedOverlayPress::Started);
        assert_eq!(timed.seconds_left(1000), 10);
        assert_eq!(timed.seconds_left(5500), 6);
        assert!(!timed.expire(10_999));
        assert!(timed.expire(11_000));

        let settings = AppSettings::default();
        let social = SocialConfig::default();
        let boundary = TrackingBoundary::from_settings(&settings);
        let model = TestOverlayBuilder::build(
            &social,
            1920.0,
            1080.0,
            Some(boundary),
            TestOverlayPart::Panel,
            Some(&settings),
        );
        let mut painter = SoftwareOverlayPainter::new();
        let bgra = painter.paint(&model, &settings, 1920, 1080);
        assert_eq!(bgra.len(), 1920 * 1080 * 4);
        let non_zero_alpha = bgra.chunks_exact(4).filter(|px| px[3] > 0).count();
        assert!(
            non_zero_alpha > 500,
            "expected visible rendered overlay pixels"
        );

        let metrics = BracketMetrics::for_box(PixelRect::new(100.0, 100.0, 120.0, 20.0), 1080.0);
        assert!(metrics.stroke > 0.0);
        let mut rast = BracketRasterizer::new();
        let mut small = vec![0u8; 400 * 400 * 4];
        let item = BracketItem {
            marker_id: 1,
            handle: "Pilot".to_string(),
            name_box: PixelRect::new(100.0, 100.0, 120.0, 20.0),
            color: OverlayColor::Cyan,
            kind: MatchKind::PlayerList,
        };
        rast.draw(&mut small, 400 * 4, 400, 400, &[item], 0.0, 0.0, 1080.0);
        assert!(small.chunks_exact(4).any(|px| px[3] > 0));

        // Verify Debug HUD & candidate box rendering when 0 targets are on screen
        let dbg_settings = AppSettings {
            debug_overlay_enabled: true,
            debug_show_candidates: true,
            ..AppSettings::default()
        };
        let round_trip_dbg = AppSettingsToml::read(&AppSettingsToml::write(&dbg_settings));
        assert!(round_trip_dbg.is_valid());
        assert!(round_trip_dbg.settings.debug_overlay_enabled);
        assert!(round_trip_dbg.settings.debug_show_candidates);

        let dbg_model = TestOverlayBuilder::build(
            &social,
            1920.0,
            1080.0,
            Some(boundary),
            TestOverlayPart::DebugHud,
            Some(&dbg_settings),
        );
        assert!(dbg_model.brackets.is_empty());
        assert!(dbg_model.panel_entries.is_empty());
        assert!(!dbg_model.is_empty());
        let dbg_bgra = painter.paint(&dbg_model, &dbg_settings, 1920, 1080);
        let dbg_visible = dbg_bgra.chunks_exact(4).filter(|px| px[3] > 0).count();
        assert!(
            dbg_visible > 500,
            "expected visible Debug HUD pixels even with 0 brackets, got {}",
            dbg_visible
        );
        // Verify premultiplied BGRA8 invariant (B, G, R <= A)
        assert!(dbg_bgra
            .chunks_exact(4)
            .all(|px| px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3]));
    }

    #[test]
    fn default_exclusions_filter_hud_tokens() {
        let excl = ExclusionList::embedded();
        assert!(!excl.is_empty());
    }

    #[test]
    fn breadcrumbs_parse_proximity_and_cloud_render() {
        use crate::breadcrumbs::{BreadcrumbTrail, RecordOutcome, SpaceCoord};
        use crate::models::ProximityZone;

        let sample_1 = "Coordinates: x:-48118694104.272797 y:48503341985.968727 z:78519.184221";
        let sample_2 = "Coordinates: x:-48118697936.058990 y:48503333471.106918 z:78517.358948";
        let c1 = SpaceCoord::parse_showlocation(sample_1).expect("valid coord 1");
        let c2 = SpaceCoord::parse_showlocation(sample_2).expect("valid coord 2");
        let d_km = c1.distance_km(&c2);
        assert!(
            (d_km - 9.337).abs() < 0.05,
            "expected ~9.34 km between pasted coordinates, got {:.3} km",
            d_km
        );

        let mut trail = BreadcrumbTrail::default();
        let t0 = Utc::now();
        // 1st point -> Clear
        match trail.record(c1, 5.0, None, t0) {
            RecordOutcome::Recorded { proximity, .. } => {
                assert_eq!(proximity.zone, ProximityZone::Clear);
            }
            RecordOutcome::Duplicate => panic!("first point should not be duplicate"),
        }
        // Duplicate (< 1m) -> ignored
        assert_eq!(trail.record(c1, 5.0, None, t0), RecordOutcome::Duplicate);
        // 2nd point ~9.34 km away with 5 km radius -> Clear (> 8 km approach)
        match trail.record(c2, 5.0, None, t0 + Duration::minutes(5)) {
            RecordOutcome::Recorded { proximity, .. } => {
                assert_eq!(proximity.zone, ProximityZone::Clear);
            }
            RecordOutcome::Duplicate => panic!("second point should be recorded"),
        }
        // 3rd point 6.5 km from c1 -> Approaching (between 5 km and 8 km)
        let c3 = SpaceCoord::new(c1.x + 6500.0, c1.y, c1.z);
        match trail.record(c3, 5.0, None, t0 + Duration::minutes(8)) {
            RecordOutcome::Recorded { proximity, .. } => {
                assert_eq!(proximity.zone, ProximityZone::Approaching);
            }
            RecordOutcome::Duplicate => panic!("third point should be recorded"),
        }
        // 4th point 2.0 km from c1 -> Visited (<= 5 km)
        let c4 = SpaceCoord::new(c1.x + 2000.0, c1.y, c1.z);
        match trail.record(c4, 5.0, None, t0 + Duration::minutes(12)) {
            RecordOutcome::Recorded { proximity, .. } => {
                assert_eq!(proximity.zone, ProximityZone::Visited);
                assert_eq!(proximity.nearest_id, Some(1));
            }
            RecordOutcome::Duplicate => panic!("fourth point should be recorded"),
        }

        // Verify SoftwareOverlayPainter renders the color cloud + radar card cleanly
        let settings = AppSettings::default();
        let social = SocialConfig::default();
        let model = TestOverlayBuilder::build(
            &social,
            1920.0,
            1080.0,
            None,
            TestOverlayPart::Breadcrumbs,
            Some(&settings),
        );
        assert!(!model.is_empty());
        let mut painter = SoftwareOverlayPainter::new();
        let bgra = painter.paint(&model, &settings, 1920, 1080);
        let visible = bgra.chunks_exact(4).filter(|px| px[3] > 0).count();
        assert!(
            visible > 2000,
            "expected visible breadcrumb cloud pixels, got {}",
            visible
        );
        // Premultiplied BGRA8 invariant (B, G, R <= A)
        assert!(bgra
            .chunks_exact(4)
            .all(|px| px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3]));

        // Verify r_DisplayInfo OCR parsing (exact lines from Star Citizen r_DisplayInfo 3)
        let di_sample = "\
            CamDir: -47 22 -129 HOF: 11\n\
            Zone: rs_refinery_pyro Pos: -17.75m 5.55m -17.46m\n\
            Zone: solarsystem_4254651523815 Pos: -16111907.1877km 45541999.9767km -90.6115km\n\
            Zone: Root Pos: -16111907.1877km 45541999.9767km -90.6115km";
        let (parsed, dual) = SpaceCoord::parse_display_info(di_sample).expect("parse display info");
        assert!(dual, "expected dual-line confirmation");
        assert!((parsed.x - (-16_111_907_187.7)).abs() < 1.0);
        assert!((parsed.y - 45_541_999_976.7).abs() < 1.0);
        assert!((parsed.z - (-90_611.5)).abs() < 1.0);

        // Verify DisplayInfoStabilizer rejects single-frame OCR glitches
        let mut stab = crate::breadcrumbs::DisplayInfoStabilizer::default();
        assert!(stab.ingest(parsed, true, t0).is_some());
        let glitch = SpaceCoord::new(parsed.x + 500_000.0, parsed.y, parsed.z);
        assert!(
            stab.ingest(glitch, false, t0 + Duration::seconds(4))
                .is_none(),
            "single-frame 500 km glitch must be rejected"
        );
        let smooth_move = SpaceCoord::new(parsed.x + 1200.0, parsed.y + 400.0, parsed.z);
        assert!(stab
            .ingest(smooth_move, false, t0 + Duration::seconds(8))
            .is_some());
        assert!(stab.should_auto_drop(parsed, 30, t0));
        assert!(!stab.should_auto_drop(smooth_move, 30, t0 + Duration::seconds(10)));
        assert!(stab.should_auto_drop(smooth_move, 30, t0 + Duration::seconds(31)));

        // Verify planetary zone-locked breadcrumbs detect backtracking even after 30 km Root rotation!
        let mut p_trail = BreadcrumbTrail::default();
        let p_local_a = SpaceCoord::new(-211_765.0, 142_670.0, -127_938.0);
        let p_local_b = SpaceCoord::new(-211_765.0, 150_670.0, -127_938.0); // 8 km away
        let root_a = SpaceCoord::new(-48_118_000_000.0, 48_502_000_000.0, -127_938.0);
        let root_rotated = SpaceCoord::new(-48_118_030_000.0, 48_502_020_000.0, -127_938.0);
        p_trail.record_with_local(root_a, Some(("pyro6".into(), p_local_a)), 5.0, None, t0);
        p_trail.record_with_local(
            root_rotated,
            Some(("pyro6".into(), p_local_b)),
            5.0,
            None,
            t0 + Duration::seconds(45),
        );
        // Now backtrack back to within 1 km of p_local_a (60s later, Root rotated 30+ km away!)
        let backtracked = SpaceCoord::new(-211_765.0, 143_670.0, -127_938.0);
        let bc_info = p_trail
            .build_overlay_info_dual(
                5.0,
                15.0,
                true,
                Some(root_rotated),
                Some(("pyro6", backtracked)),
                t0 + Duration::seconds(60),
            )
            .expect("planetary breadcrumb overlay info");
        assert_eq!(
            bc_info.zone,
            ProximityZone::Visited,
            "backtracking over a planetary breadcrumb must trigger VISITED even after Root rotation"
        );
    }

    #[test]
    fn morphology_exactness_and_frame_latency() {
        use crate::detection::image_ops::{box_blur, dilate_rect, erode_rect, BgrImage, GrayImage};
        use crate::detection::{NameplateDetector, NameplateDetectorSettings, NameplateReader};
        use std::time::Instant;

        // 1. Verify exact bit-for-bit match against brute-force 2D window on an irregular size (203x67)
        let w = 203usize;
        let h = 67usize;
        let mut data = vec![0u8; w * h];
        for (i, b) in data.iter_mut().enumerate() {
            *b = ((i * 73 + 19) % 251) as u8;
        }
        let img = GrayImage::from_vec(w, h, data);
        let kw = 11usize;
        let kh = 7usize;
        let rx = (kw / 2) as isize;
        let ry = (kh / 2) as isize;

        let fast_dil = dilate_rect(&img, kw, kh);
        let fast_ero = erode_rect(&img, kw, kh);
        let fast_blur = box_blur(&img, 9);

        for y in 0..h {
            for x in 0..w {
                let mut mx = 0u8;
                let mut mn = 255u8;
                for dy in 0..kh as isize {
                    let sy = (y as isize - ry + dy).clamp(0, h as isize - 1) as usize;
                    for dx in 0..kw as isize {
                        let sx = (x as isize - rx + dx).clamp(0, w as isize - 1) as usize;
                        let v = img.data[sy * w + sx];
                        mx = mx.max(v);
                        mn = mn.min(v);
                    }
                }
                assert_eq!(fast_dil.data[y * w + x], mx);
                assert_eq!(fast_ero.data[y * w + x], mn);

                let y0 = (y as isize - 4).clamp(0, h as isize - 1) as usize;
                let y1 = (y as isize + 4).clamp(0, h as isize - 1) as usize + 1;
                let x0 = (x as isize - 4).clamp(0, w as isize - 1) as usize;
                let x1 = (x as isize + 4).clamp(0, w as isize - 1) as usize + 1;
                let mut sum = 0u32;
                for sy in y0..y1 {
                    for sx in x0..x1 {
                        sum += img.data[sy * w + sx] as u32;
                    }
                }
                let area = ((y1 - y0) * (x1 - x0)) as u32;
                let expected_blur = ((sum + area / 2) / area) as u8;
                assert_eq!(fast_blur.data[y * w + x], expected_blur);
            }
        }

        // 2. Measure warm steady-state detector scan latency on a 1920x1080 Star Citizen HUD frame
        let mut det = NameplateDetector::new(
            NameplateReader::default(),
            NameplateDetectorSettings {
                region_left: 0.15,
                region_right: 0.85,
                region_top: 0.15,
                region_bottom: 0.85,
                ..NameplateDetectorSettings::default()
            },
        );
        let mut frame_1080 = BgrImage::new(1920, 1080);
        // Simulate dark space + bright HUD text speckles
        for i in (0..frame_1080.data.len()).step_by(3) {
            frame_1080.data[i] = 18;
            frame_1080.data[i + 1] = 22;
            frame_1080.data[i + 2] = 26;
        }
        for x in 860..980 {
            for y in [500usize, 502, 518, 520] {
                let idx = (y * 1920 + x) * 3;
                frame_1080.data[idx] = 235;
                frame_1080.data[idx + 1] = 240;
                frame_1080.data[idx + 2] = 240;
            }
        }
        // Warm up reusable buffers
        let _ = det.find_candidates(&frame_1080);
        let iters = 10;
        let t0 = Instant::now();
        for _ in 0..iters {
            let _ = det.find_candidates(&frame_1080);
        }
        let avg_ms = t0.elapsed().as_secs_f64() * 1000.0 / iters as f64;
        println!("1080p warm find_candidates avg: {:.2} ms/frame", avg_ms);
    }

    #[test]
    fn waypoints_storage_guidance_and_hud_render() {
        use crate::breadcrumbs::SpaceCoord;
        use crate::waypoints::{
            format_distance_m, HeadingTracker, WaypointGuidance, WaypointStore,
        };

        // 1. Hotkey & settings roundtrip for Alt+Shift+P
        let mut settings = AppSettings::default();
        assert_eq!(settings.hotkey_save_waypoint, "Alt+Shift+P");
        assert!(settings.waypoints_enabled);
        assert!(settings.waypoints_show_ring_marker);
        let chord = HotkeyText::parse(&settings.hotkey_save_waypoint).expect("Alt+Shift+P valid");
        assert!(chord.alt && chord.shift && !chord.ctrl && chord.key == "P");
        settings.waypoints_show_ring_marker = false;
        let round = AppSettingsToml::read(&AppSettingsToml::write(&settings));
        assert!(round.is_valid());
        assert_eq!(round.settings.hotkey_save_waypoint, "Alt+Shift+P");
        assert!(!round.settings.waypoints_show_ring_marker);

        // 2. Zone parsing from r_DisplayInfo
        let di = "\
            CamDir: -47 22 -129 HOF: 11\n\
            Zone: rs_refinery_pyro Pos: -17.75m 5.55m -17.46m\n\
            Zone: solarsystem_4254651523815 Pos: -16111907.1877km 45541999.9767km -90.6115km\n\
            Zone: Root Pos: -16111907.1877km 45541999.9767km -90.6115km";
        assert_eq!(
            SpaceCoord::parse_display_info_zone(di).as_deref(),
            Some("rs_refinery_pyro")
        );

        // 3. WaypointStore CRUD + JSON import/export
        let t0 = Utc::now();
        let mut store = WaypointStore::default();
        let target_coord = SpaceCoord::new(0.0, 10_000.0, 500.0);
        let wp = store.add_point(
            target_coord,
            None,
            Some("solarsystem_test".to_string()),
            Some("Rich Quantainium".to_string()),
            Some(OverlayColor::Yellow),
            t0,
        );
        assert_eq!(wp.name, "POI #1");
        assert!(store.rename(&wp.id, "Quant Rock Alpha"));
        assert!(store.set_active(Some(&wp.id)));
        assert_eq!(store.active().unwrap().name, "Quant Rock Alpha");

        let exported = store.export_json().expect("export json");
        let mut imported_store = WaypointStore::default();
        assert_eq!(imported_store.import_json(&exported).unwrap(), 1);
        assert_eq!(imported_store.import_json(&exported).unwrap(), 0); // deduplicated by id
        assert_eq!(imported_store.waypoints[0].name, "Quant Rock Alpha");

        // 4. Distance formatting (10.0 km -> 9.9 km -> 420 m)
        assert_eq!(format_distance_m(10_000.0), "10.0 km");
        assert_eq!(format_distance_m(9_900.0), "9.9 km");
        assert_eq!(format_distance_m(420.0), "420 m");

        // 5. HeadingTracker + directional math
        let active_wp = store.active().unwrap().clone();
        let mut heading = HeadingTracker::default();
        // Flying straight North (+Y) toward target at (0, 10_000, 500): 10.0 km -> 9.9 km
        heading.push(SpaceCoord::new(0.0, 0.0, 0.0), 1000.0);
        heading.push(SpaceCoord::new(0.0, 100.0, 0.0), 2500.0);
        let g_ahead = WaypointGuidance::build_overlay_info(
            &active_wp,
            Some(SpaceCoord::new(0.0, 100.0, 0.0)),
            &heading,
            true,
            2500.0,
        );
        assert_eq!(g_ahead.distance_text, "9.9 km");
        assert_eq!(g_ahead.steering_text, "ON COURSE");
        assert!(g_ahead.rel_yaw_rad.unwrap().abs() < 0.02);
        assert!(g_ahead.vertical_text.contains("UP"));
        assert!(g_ahead.closing_mps.unwrap() > 50.0);
        assert!(g_ahead.show_ring_marker);

        // Flying North (+Y), target is directly East (+X): should steer RIGHT 90 deg (rel_yaw ~ -pi/2)
        let east_wp = store.add_point(
            SpaceCoord::new(5_000.0, 100.0, 0.0),
            Some("East".into()),
            None,
            None,
            None,
            t0,
        );
        let g_right = WaypointGuidance::build_overlay_info(
            &east_wp,
            Some(SpaceCoord::new(0.0, 100.0, 0.0)),
            &heading,
            true,
            2500.0,
        );
        assert!(g_right.rel_yaw_rad.unwrap() < -1.4);
        assert!(g_right.steering_text.starts_with("RIGHT"));

        // Target directly West (-X): should steer LEFT 90 deg (rel_yaw ~ +pi/2)
        let west_wp = store.add_point(
            SpaceCoord::new(-5_000.0, 100.0, 0.0),
            Some("West".into()),
            None,
            None,
            None,
            t0,
        );
        let g_left = WaypointGuidance::build_overlay_info(
            &west_wp,
            Some(SpaceCoord::new(0.0, 100.0, 0.0)),
            &heading,
            true,
            2500.0,
        );
        assert!(g_left.rel_yaw_rad.unwrap() > 1.4);
        assert!(g_left.steering_text.starts_with("LEFT"));

        // Arrived (< 150 m)
        let g_arrived = WaypointGuidance::build_overlay_info(
            &active_wp,
            Some(SpaceCoord::new(0.0, 9_950.0, 500.0)),
            &heading,
            true,
            2500.0,
        );
        assert!(g_arrived.arrived);
        assert_eq!(g_arrived.steering_text, "ARRIVED AT POI");
        assert!(!g_arrived.show_ring_marker);

        // 6. Verify SoftwareOverlayPainter renders Waypoint HUD card + floating ring marker cleanly
        let render_settings = AppSettings::default();
        let social = SocialConfig::default();
        let wp_model = TestOverlayBuilder::build(
            &social,
            1920.0,
            1080.0,
            None,
            TestOverlayPart::Waypoint,
            Some(&render_settings),
        );
        assert!(!wp_model.is_empty());
        let mut painter = SoftwareOverlayPainter::new();
        let bgra = painter.paint(&wp_model, &render_settings, 1920, 1080);
        let visible = bgra.chunks_exact(4).filter(|px| px[3] > 0).count();
        assert!(
            visible > 2000,
            "expected visible waypoint HUD pixels, got {}",
            visible
        );
        assert!(bgra
            .chunks_exact(4)
            .all(|px| px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3]));

        // 7. Planetary / Moon Zone Locking (zero rotation drift on planets, moons & orbital stations!)
        use crate::breadcrumbs::{zones_match, LocalBodyStabilizer};
        use crate::waypoints::LivePositionFix;

        let ruin_di = "\
            CamDir: -47 22 -129 HOF: 11\n\
            Zone: MISC_Prospector_952196486641 Pos: -0.01m 9.73m 0.41m\n\
            Zone: rs_int_p6leo_ruinstation Pos: -22.50m -19.79m -7.33m\n\
            Zone: pyr06 Pos: -357.1000km 302.1084km -100.0020km\n\
            Zone: solarsystem_4254651523815 Pos: -48118100.5300km 48502730.2570km -100.0270km\n\
            Zone: Root Pos: -48118100.5300km 48502730.2570km -100.0270km";
        let telem = SpaceCoord::parse_display_info_telemetry(ruin_di);
        assert!(telem.root.is_some());
        let (disp_z, norm_z, body_c) = telem.body.expect("expected pyr06 planetary container");
        assert_eq!(disp_z, "pyr06");
        assert_eq!(norm_z, "pyro6");
        assert!(zones_match("pyr06", "pyro6"));
        assert!((body_c.x - (-357_100.0)).abs() < 1.0);

        // LocalBodyStabilizer requires 2 consistent frames then tracks smoothly & rejects glitches
        let mut bstab = LocalBodyStabilizer::default();
        assert!(bstab
            .ingest(disp_z.clone(), norm_z.clone(), body_c, t0)
            .is_none());
        let (_, _, acc_body) = bstab
            .ingest(
                "pyro6".to_string(),
                "pyro6".to_string(),
                body_c,
                t0 + Duration::milliseconds(500),
            )
            .expect("2nd consistent frame should lock planetary body");
        // Single-frame 3 km OCR digit typo in 200 ms must be rejected!
        assert!(bstab
            .ingest(
                "pyro6".to_string(),
                "pyro6".to_string(),
                SpaceCoord::new(body_c.x + 3_000.0, body_c.y, body_c.z),
                t0 + Duration::milliseconds(700)
            )
            .is_none());

        // Save POI on Ruin Station (inside pyr06), then simulate the planet rotating 25 km in Root Pos
        // while the player stands completely still on Ruin Station (local pyr06 pos unchanged!):
        let ruin_wp = store.add_point_with_local(
            telem.root.unwrap().0,
            Some((disp_z.clone(), acc_body)),
            Some("Ruin Pad".to_string()),
            Some(disp_z.clone()),
            None,
            Some(OverlayColor::Cyan),
            t0,
        );
        let rotated_root = SpaceCoord::new(
            telem.root.unwrap().0.x + 25_000.0,
            telem.root.unwrap().0.y - 12_000.0,
            telem.root.unwrap().0.z,
        );
        let fix_on_station = LivePositionFix {
            root: Some(rotated_root),
            body: Some(("pyro6".to_string(), "pyro6".to_string(), acc_body)),
            display_zone: Some("pyro6".to_string()),
        };
        let empty_heading = HeadingTracker::default();
        let locked_guidance = WaypointGuidance::build_overlay_info_dual(
            &ruin_wp,
            &fix_on_station,
            &empty_heading,
            &empty_heading,
            true,
            5000.0,
        );
        assert!(
            locked_guidance.zone_lock.is_some(),
            "expected planetary zone lock"
        );
        assert!(
            locked_guidance.arrived,
            "standing still in pyr06 must stay 0m (ARRIVED) despite 25 km Root rotation!"
        );
        assert!(locked_guidance.distance_m < 1.0);

        // 8. Spherical Planetary ENU Tangent-Plane Navigation (equator test on R = 500 km planet)
        let eq_cur = SpaceCoord::new(500_000.0, 0.0, 0.0);
        // Target 10 km East along equator (+Y at (R, 0, 0)), 500 m higher altitude (+X is radial Up!)
        let eq_east_wp = store.add_point_with_local(
            SpaceCoord::new(0.0, 0.0, 0.0),
            Some((
                "pyro6".to_string(),
                SpaceCoord::new(500_500.0, 10_000.0, 0.0),
            )),
            Some("Equator East".to_string()),
            Some("pyro6".to_string()),
            None,
            Some(OverlayColor::Cyan),
            t0,
        );
        // Ship flying straight North along the equator (+Z is North at the equator!)
        let mut eq_heading = HeadingTracker::default();
        eq_heading.push(SpaceCoord::new(500_000.0, 0.0, -200.0), 1000.0);
        eq_heading.push(eq_cur, 1500.0);
        let eq_fix = LivePositionFix {
            root: None,
            body: Some(("pyro6".to_string(), "pyro6".to_string(), eq_cur)),
            display_zone: Some("pyro6".to_string()),
        };
        let eq_g = WaypointGuidance::build_overlay_info_dual(
            &eq_east_wp,
            &eq_fix,
            &empty_heading,
            &eq_heading,
            true,
            1500.0,
        );
        assert!(
            (eq_g.world_bearing_deg - 90.0).abs() < 1.5,
            "expected ~090° East compass heading on equator, got {:.1}°",
            eq_g.world_bearing_deg
        );
        assert!(
            eq_g.steering_text.starts_with("RIGHT"),
            "flying North (+Z) toward East (+Y) on equator must steer RIGHT, got {}",
            eq_g.steering_text
        );
        assert!(
            eq_g.vertical_text.contains("UP"),
            "500 m higher radial altitude must show UP, got {}",
            eq_g.vertical_text
        );
        assert!(
            eq_g.subline.contains("HDG 090°"),
            "planetary subline should include zero-lag compass heading, got {}",
            eq_g.subline
        );
    }

    #[test]
    fn players_orgs_toggle_settings_and_pipeline_bypass() {
        use crate::config::DataPaths;
        use crate::lookup::RsiClient;
        use crate::pipeline::{CapturedFrame, LiveDetectionPipeline};
        use parking_lot::RwLock;
        use std::sync::Arc;

        // 1. Default settings enable Players & Orgs with Shift+F11 hotkey
        let defaults = AppSettings::default();
        assert!(defaults.players_orgs_enabled);
        assert_eq!(defaults.hotkey_toggle_players_orgs, "Shift+F11");

        // 2. Legacy settings.txt without `players_orgs_enabled` defaults to enabled
        let legacy_loaded = AppSettingsToml::read("format_version = 1\noverlay_disabled = false\n");
        assert!(legacy_loaded.is_valid());
        assert!(legacy_loaded.settings.players_orgs_enabled);

        // 3. Explicit false + custom hotkey round-trips cleanly
        let custom = AppSettings {
            players_orgs_enabled: false,
            hotkey_toggle_players_orgs: "Ctrl+Shift+P".to_string(),
            ..AppSettings::default()
        };
        let reloaded = AppSettingsToml::read(&AppSettingsToml::write(&custom));
        assert!(reloaded.is_valid(), "errors: {:?}", reloaded.errors);
        assert!(!reloaded.settings.players_orgs_enabled);
        assert_eq!(reloaded.settings.hotkey_toggle_players_orgs, "Ctrl+Shift+P");

        // 4. LiveDetectionPipeline skips all nameplate scanning when `players_orgs_enabled` is false
        let paths = DataPaths::resolve(false);
        let cache = Arc::new(CacheDatabase::open_in_memory().expect("in-memory sqlite"));
        let notes = Arc::new(RwLock::new(DossierNotes::new(
            DossierNotes::DEFAULT_MAX_FILE_BYTES,
        )));
        let client = Arc::new(RsiClient::new().expect("rsi client"));
        let mut pipeline = LiveDetectionPipeline::new(paths, cache, notes, client);

        // Non-black synthetic frame with bright high-contrast stripes that would normally generate candidate boxes
        let (w, h) = (640usize, 360usize);
        let mut bgr = vec![30u8; w * h * 3];
        for y in 120..136 {
            for x in 200..320 {
                let idx = (y * w + x) * 3;
                bgr[idx] = 255;
                bgr[idx + 1] = 255;
                bgr[idx + 2] = 255;
            }
        }
        let frame = CapturedFrame::from_bgr8(w, h, bgr, 1_000.0);
        let social = SocialConfig::default();
        let off_settings = AppSettings {
            players_orgs_enabled: false,
            mining_disabled: true,
            ..AppSettings::default()
        };

        let snap = pipeline.process_frame(&frame, &off_settings, &social);
        assert_eq!(snap.stats.candidate_boxes, 0);
        assert_eq!(snap.stats.candidate_pairs, 0);
        assert_eq!(snap.stats.ocr_reads, 0);
        assert_eq!(snap.stats.confirmed_tracks, 0);
        assert_eq!(snap.stats.tentative_tracks, 0);
        assert!(snap.model.brackets.is_empty());
        assert!(snap.model.panel_entries.is_empty());
        assert!(snap.roster.is_empty());
    }

    #[test]
    fn breadcrumb_radar_scale_presets_filter_and_render() {
        use crate::breadcrumbs::{BreadcrumbTrail, SpaceCoord};

        // 1. Default Auto = 0 -> effective 3x visited_radius_km (5 * 3 = 15 km)
        let mut s = AppSettings::default();
        assert_eq!(s.breadcrumbs_radar_range_km, 0);
        assert!((s.effective_breadcrumb_radar_km() - 15.0).abs() < 0.01);

        // 2. Step zoom out / zoom in across 1..1000 km presets
        assert_eq!(AppSettings::step_breadcrumb_radar_km(0, 5, 1, false), 25);
        assert_eq!(AppSettings::step_breadcrumb_radar_km(0, 5, -1, false), 10);
        assert_eq!(AppSettings::step_breadcrumb_radar_km(10, 5, -1, false), 5);
        assert_eq!(AppSettings::step_breadcrumb_radar_km(1, 5, -1, false), 1);
        assert_eq!(AppSettings::step_breadcrumb_radar_km(100, 5, 1, false), 250);
        assert_eq!(AppSettings::step_breadcrumb_radar_km(1000, 5, 1, false), 1000);
        assert_eq!(AppSettings::step_breadcrumb_radar_km(1000, 5, 1, true), 0);

        // 3. Settings TOML round-trip
        s.breadcrumbs_radar_range_km = 100;
        let reloaded = AppSettingsToml::read(&AppSettingsToml::write(&s));
        assert!(reloaded.is_valid(), "errors: {:?}", reloaded.errors);
        assert_eq!(reloaded.settings.breadcrumbs_radar_range_km, 100);
        assert_eq!(reloaded.settings.hotkey_radar_zoom_in, "Alt+Shift+[");
        assert_eq!(reloaded.settings.hotkey_radar_zoom_out, "Alt+Shift+]");

        // 4. Wide scale (500 km) retains distant points beyond legacy 250 km cap and renders cleanly
        let mut trail = BreadcrumbTrail::default();
        let t0 = Utc::now();
        let p0 = SpaceCoord::new(0.0, 0.0, 0.0);
        let p_far = SpaceCoord::new(400_000.0, 0.0, 0.0); // 400 km away
        trail.record(p0, 5.0, None, t0);
        trail.record(p_far, 5.0, None, t0 + Duration::minutes(5));

        let info_500 = trail
            .build_overlay_info(5.0, 500.0, true, Some(p0), t0 + Duration::minutes(6))
            .expect("overlay info");
        assert!((info_500.radar_range_km - 500.0).abs() < 0.01);
        assert!(
            info_500.points.iter().any(|pt| (pt.dx_km - 400.0).abs() < 0.1),
            "500 km radar scale must retain 400 km distant point"
        );

        let social = SocialConfig::default();
        let mut painter = SoftwareOverlayPainter::new();
        for km in [1, 10, 100, 1000] {
            let preview_settings = AppSettings {
                breadcrumbs_radar_range_km: km,
                ..AppSettings::default()
            };
            let model = TestOverlayBuilder::build(
                &social,
                1920.0,
                1080.0,
                None,
                TestOverlayPart::Breadcrumbs,
                Some(&preview_settings),
            );
            let bgra = painter.paint(&model, &preview_settings, 1920, 1080);
            assert!(bgra.chunks_exact(4).any(|px| px[3] > 0));
            assert!(bgra
                .chunks_exact(4)
                .all(|px| px[0] <= px[3] && px[1] <= px[3] && px[2] <= px[3]));
        }
    }
}
