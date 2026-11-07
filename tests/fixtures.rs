//! Reference print jobs: tests/fixtures/*.png are fitted labels, *.bin the bytes the
//! encoder version verified on a real HP4 produced for them. They must not change.

use open_hp4_printer::media;
use open_hp4_printer::protocol::Encoder;

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[test]
fn print_jobs_match_reference() {
    let spec: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/commands.json")).unwrap();
    for case in spec["cases"].as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let media = media::find(case["media"].as_str().unwrap()).unwrap();
        let img = image::open(format!("tests/fixtures/{name}.png"))
            .unwrap()
            .to_rgb8();
        let job = open_hp4_printer::build_job(
            &mut Encoder::default(),
            &img,
            &media,
            case["density"].as_u64().unwrap() as u8,
            case["first"].as_bool().unwrap(),
            case["last"].as_bool().unwrap(),
        )
        .unwrap();
        let expected = std::fs::read(format!("tests/fixtures/{name}.bin")).unwrap();
        assert!(
            job == expected,
            "{name}: {} bytes vs {} expected",
            job.len(),
            expected.len()
        );
    }
}

#[test]
fn commands_match_reference() {
    let spec: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/commands.json")).unwrap();
    let check =
        |key: &str, got: Vec<u8>| assert_eq!(hex(&got), spec[key].as_str().unwrap(), "{key}");
    check("status", Encoder::default().get_status());
    check("serial", Encoder::default().get_serial());
    check("firmware", Encoder::default().get_firmware());
    check("auto_off_never", Encoder::default().auto_off(0));
    check("auto_off_1h", Encoder::default().auto_off(6));
    check("feed_80", Encoder::default().feed(80));
}
