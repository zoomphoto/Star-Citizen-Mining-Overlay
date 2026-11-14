pub mod config;
pub mod detection;
pub mod lookup;
pub mod mining;
pub mod models;
pub mod overlay;
pub mod pipeline;

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

        let fetched = db.get_player("citizenone").expect("case-insensitive lookup");
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
        assert_eq!(pin.len(), DigitTemplates::PIN_WIDTH * DigitTemplates::PIN_HEIGHT);
        let blank_patch = vec![0.0f32; DigitTemplates::WIDTH * DigitTemplates::HEIGHT];
        let (digit, _score, _all) = DigitTemplates::match_patch(&blank_patch, 0.08, false);
        assert!(digit.is_ascii_digit());
    }

    #[test]
    fn hungarian_solver_and_lookalikes() {
        let cost = [
            10.0, 1.0, 9.0, 9.0, // row 0 -> col 1 (1.0)
            9.0, 9.0, 2.0, 8.0,  // row 1 -> col 2 (2.0)
            3.0, 9.0, 9.0, 7.0,  // row 2 -> col 0 (3.0)
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
        assert!(non_zero_alpha > 500, "expected visible rendered overlay pixels");

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
    }

    #[test]
    fn default_exclusions_filter_hud_tokens() {
        let excl = ExclusionList::embedded();
        assert!(!excl.is_empty());
    }
}
