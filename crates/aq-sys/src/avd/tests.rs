use aq_core::devspec::DeviceSpec;

use super::*;

const LIST: &str = r#"Available devices definitions:
id: 28 or "pixel_6"
    Name: Pixel 6
    OEM : Google
---------
id: 31 or "pixel_7"
    Name: Pixel 7
---------
id: 34 or "pixel_tablet"
    Name: Pixel Tablet
---------
id: 12 or "Nexus 5X"
    Name: Nexus 5X
---------
id: 43 or "wearos_rect"
    Name: Wear OS Rectangular
    OEM : Google
    Tag : android-wear
"#;

fn img(api: &str, tag: &str, abi: &str) -> Image {
    Image { api: api.into(), tag: tag.into(), abi: abi.into() }
}

fn images() -> Vec<Image> {
    vec![
        img("35", "google_apis", "arm64-v8a"),
        img("36", "google_apis_playstore", "x86_64"),
        img("37.0", "google_apis_playstore_ps16k", "arm64-v8a"),
        img("37.0", "google_apis", "arm64-v8a"),
    ]
}

fn def() -> AvdConf {
    AvdConf { sysdir: "system-images/android-35/google_apis/arm64-v8a/".into(), device: "pixel_6".into(), ram: "4096M".into() }
}

fn spec(v: Option<&str>, m: Option<&str>) -> DeviceSpec {
    DeviceSpec { version: v.map(String::from), model: m.map(String::from) }
}

fn run(v: Option<&str>, m: Option<&str>) -> Result<Plan, String> {
    let profiles = parse_profiles(LIST);
    plan(&spec(v, m), &def(), &images(), "arm64-v8a", Some(&profiles))
}

#[test]
fn parses_profiles_with_tags_and_spaces() {
    let p = parse_profiles(LIST);
    assert_eq!(p.len(), 5);
    assert_eq!(p[3], Profile { id: "Nexus 5X".into(), kind: "phone" });
    assert_eq!(p[4], Profile { id: "wearos_rect".into(), kind: "wear" });
}

// CASE-HOLD-16, CASE-HOLD-19
#[test]
fn defaults_come_from_the_configured_avd() {
    let p = run(None, None).unwrap();
    assert_eq!(
        (p.image.package().as_str(), p.device.as_str(), p.ram.as_str()),
        ("system-images;android-35;google_apis;arm64-v8a", "pixel_6", "4096M")
    );
    let p = run(None, Some("pixel_tablet")).unwrap();
    assert_eq!((p.image.api.as_str(), p.device.as_str(), p.ram.as_str()), ("35", "pixel_tablet", "4096M"));
    let p = run(Some("35"), Some("Nexus 5X")).unwrap();
    assert_eq!(p.device, "Nexus 5X");
}

// CASE-HOLD-19
#[test]
fn version_picks_a_runnable_image_preferring_google_apis() {
    let p = run(Some("37.0"), None).unwrap();
    assert_eq!((p.image.tag.as_str(), p.device.as_str(), p.ram.as_str()), ("google_apis", "pixel_6", "4096M"));
}

// CASE-HOLD-14
#[test]
fn unfit_combinations_say_what_is_missing() {
    let e = run(Some("34"), None).unwrap_err();
    assert!(
        e.contains("no arm64 Android API 34")
            && e.contains("installed: 35, 37.0")
            && e.contains("sdkmanager \"system-images;android-34;google_apis;arm64-v8a\""),
        "{e}"
    );
    let e = run(Some("36"), None).unwrap_err();
    assert!(e.contains("installed only as x86_64") && e.contains("installed arm64: 35, 37.0"), "{e}");
    let e = run(Some("37"), None).unwrap_err();
    assert!(e.contains("Write the full API level, e.g. android-emu@37.0") && !e.contains("sdkmanager"), "{e}");
    let e = run(Some("35"), Some("nope")).unwrap_err();
    assert!(e.contains("no Android device profile named nope; e.g. pixel_6, pixel_7, pixel_tablet"), "{e}");
    let e = run(Some("35"), Some("wearos_rect")).unwrap_err();
    assert!(
        e.contains("wearos_rect is a Wear OS profile") && e.contains("Pick a phone or tablet profile, e.g. android-emu@35:pixel_6"),
        "{e}"
    );
    let missing = plan(&spec(None, None), &def(), &images()[1..], "arm64-v8a", None).unwrap_err();
    assert!(missing.contains("[android-emu] avd in config.toml is not installed"), "{missing}");
}

#[test]
fn set_ram_replaces_the_line_and_remove_only_touches_aq_avds() {
    let d = tempfile::tempdir().unwrap();
    let avd = d.path().join("aq-x-hd_1-1.avd");
    std::fs::create_dir_all(&avd).unwrap();
    std::fs::write(avd.join("config.ini"), "hw.ramSize=1536M\nhw.device.name=pixel_6\n").unwrap();
    std::fs::write(d.path().join("aq-x-hd_1-1.ini"), "path=x\n").unwrap();
    set_ram(&avd, "4096M").unwrap();
    let text = std::fs::read_to_string(avd.join("config.ini")).unwrap();
    assert!(text.contains("hw.ramSize=4096M") && !text.contains("1536M") && text.contains("pixel_6"), "{text}");
    assert_eq!(names(d.path()), ["aq-x-hd_1-1"]);
    assert!(remove(d.path(), "../etc").is_err());
    remove(d.path(), "aq-x-hd_1-1").unwrap();
    assert!(!avd.exists() && names(d.path()).is_empty());
    assert!(remove(d.path(), "aq-x-hd_1-1").is_err(), "删不存在的报错");
}
