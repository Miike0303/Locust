//! Wire fixtures deliberately do not use the production Marshal writer.
use std::{collections::HashMap, fs, path::PathBuf};

use locust_core::{extraction::FormatPlugin, models::StringEntry};
use locust_formats::rpgmaker_vxa::{MarshalValue, RpgMakerVxaPlugin};

#[derive(Default)]
struct Wire {
    bytes: Vec<u8>,
    symbols: HashMap<String, usize>,
    objects: usize,
}

impl Wire {
    fn new() -> Self {
        Self {
            bytes: vec![4, 8],
            ..Self::default()
        }
    }
    fn packed(&mut self, mut n: i64) {
        if n == 0 {
            self.bytes.push(0);
        } else if (1..123).contains(&n) {
            self.bytes.push((n + 5) as u8);
        } else if (-123..0).contains(&n) {
            self.bytes.push((n - 5) as u8);
        } else {
            let negative = n < 0;
            let start = self.bytes.len();
            self.bytes.push(0);
            loop {
                self.bytes.push(n as u8);
                n >>= 8;
                if n == 0 || n == -1 {
                    break;
                }
            }
            let count = (self.bytes.len() - start - 1) as i8;
            self.bytes[start] = if negative { -count } else { count } as u8;
        }
    }
    fn blob(&mut self, bytes: &[u8]) {
        self.packed(bytes.len() as i64);
        self.bytes.extend_from_slice(bytes);
    }
    fn sym(&mut self, name: &str) {
        if let Some(&id) = self.symbols.get(name) {
            self.bytes.push(b';');
            self.packed(id as i64);
        } else {
            self.symbols.insert(name.into(), self.symbols.len());
            self.bytes.push(b':');
            self.blob(name.as_bytes());
        }
    }
    fn register(&mut self) -> usize {
        let id = self.objects;
        self.objects += 1;
        id
    }
    fn array(&mut self, count: usize) -> usize {
        self.bytes.push(b'[');
        self.packed(count as i64);
        self.register()
    }
    fn object(&mut self, class: &str, count: usize) -> usize {
        self.bytes.push(b'o');
        let id = self.register();
        self.sym(class);
        self.packed(count as i64);
        id
    }
    fn string(&mut self, text: &str) -> usize {
        self.bytes.push(b'"');
        self.blob(text.as_bytes());
        self.register()
    }
    fn int(&mut self, n: i64) {
        self.bytes.push(b'i');
        self.packed(n);
    }
    fn link(&mut self, id: usize) {
        self.bytes.push(b'@');
        self.packed(id as i64);
    }
    fn command(&mut self, code: i64, count: usize) {
        self.object("RPG::EventCommand", 3);
        self.sym("@code");
        self.int(code);
        self.sym("@indent");
        self.int(0);
        self.sym("@parameters");
        self.array(count);
    }
}

fn roundtrip(bytes: &[u8]) {
    let parsed = MarshalValue::parse(bytes).expect("fixture must parse completely");
    assert_eq!(parsed.serialize(), bytes, "Marshal wire data changed");
}

macro_rules! wire_test {
    ($name:ident, $bytes:expr) => {
        #[test]
        fn $name() {
            roundtrip($bytes);
        }
    };
}

wire_test!(booleans_and_nil, b"\x04\x08[\x08TF0");
wire_test!(
    object_link_and_self_cycle,
    b"\x04\x08[\x08o:\x06X\x06:\x07@x@\x06@\x06@\x00"
);
wire_test!(string_link, b"\x04\x08[\x07\"\x08abc@\x06");
wire_test!(binary_string, b"\x04\x08\"\x08\xff\x00\x80");
wire_test!(float_one, b"\x04\x08[\x07f\x081.0T");
wire_test!(float_inf, b"\x04\x08[\x07f\x08infF");
wire_test!(float_minus_inf, b"\x04\x08[\x07f\x09-inf0");
wire_test!(float_nan, b"\x04\x08[\x07f\x08nanT");
wire_test!(float_legacy_mantissa, b"\x04\x08[\x07f\x0a1.1\x00\x80T");
wire_test!(
    bignum_positive,
    b"\x04\x08[\x07l+\x09\x01\x00\x02\x00\x03\x00\xff\xffT"
);
wire_test!(
    bignum_negative,
    b"\x04\x08[\x07l-\x09\x01\x00\x02\x00\x03\x00\xff\xffT"
);
wire_test!(
    hash_default_and_self_link,
    b"\x04\x08[\x07}\x06i\x06T@\x06F"
);
wire_test!(
    struct_members,
    b"\x04\x08[\x07S:\x06X\x07:\x06zf\x061:\x06aT0"
);
wire_test!(user_class, b"\x04\x08[\x07C:\x06X[\x06@\x06T");
wire_test!(extended_object, b"\x04\x08[\x07e:\x06Mo:\x06X\x000");
wire_test!(user_marshal_cycle, b"\x04\x08[\x07U:\x06X[\x06@\x06T");
wire_test!(data_object_cycle, b"\x04\x08[\x07d:\x06X[\x06@\x06T");
wire_test!(regexp_flags, b"\x04\x08[\x07/\x08a+b\x15T");
wire_test!(class_ref, b"\x04\x08[\x07c\x0bObject@\x06");
wire_test!(module_ref, b"\x04\x08[\x07m\x0bKernel@\x06");
wire_test!(old_module_ref, b"\x04\x08[\x07M\x0bKernel@\x06");
wire_test!(encoding_utf8, b"\x04\x08[\x07I\"\x08abc\x06:\x06ET@\x06");
wire_test!(encoding_ascii, b"\x04\x08[\x07I\"\x08abc\x06:\x06EF@\x06");
wire_test!(
    encoding_named,
    b"\x04\x08[\x08I\"\x08abc\x06:\x0dencoding\"\x0eShift_JIS@\x06@\x07"
);
wire_test!(ivar_order, b"\x04\x08o:\x06X\x08:\x07@zT:\x07@aF:\x07@m0");
wire_test!(
    encoded_symbol,
    b"\x04\x08[\x07I:\x07\xc3\xa9\x06:\x06ET;\x00"
);
// _load data ivars are read BEFORE registering the resulting user object.
wire_test!(
    user_defined_registration,
    b"\x04\x08[\x08Iu:\x06X\x06x\x06:\x0dencoding\"\x0eShift_JIS@\x06@\x07"
);
wire_test!(
    negative_packed_integer,
    b"\x04\x08[\x08i\xff\x00i\xfe\x00\x00i\xfd\x00\x00\x00"
);
// XP files can have a redundant sign byte (-160, from the measured game).
wire_test!(
    xp_negative_integer_wire_spelling,
    b"\x04\x08[\x07i\xfe\x60\xffT"
);
wire_test!(negative_four_byte_fixnum, b"\x04\x08i\xfc\x00\x00\x00\x00");

#[test]
fn lengths_and_nesting_are_bounded() {
    for tag in [b'[', b'{', b'"', b'f'] {
        let mut bytes = vec![4, 8, tag, 4];
        bytes.extend_from_slice(&u32::MAX.to_le_bytes());
        assert!(MarshalValue::parse(&bytes).is_err());
    }
    let mut bytes = vec![4, 8];
    for _ in 0..256 {
        bytes.extend_from_slice(b"[\x06");
    }
    bytes.push(b'0');
    assert!(MarshalValue::parse(&bytes).is_err());
}

fn database(name: &str, encoding: u8) -> Vec<u8> {
    let mut w = Wire::new();
    w.array(2);
    w.bytes.push(b'0');
    w.object("RPG::Actor", 3);
    w.sym("@name");
    w.bytes.push(b'I');
    w.string(name);
    w.packed(2);
    w.sym("E");
    w.bytes.push(encoding);
    w.sym("@custom");
    w.int(17);
    w.sym("@features");
    w.array(1);
    w.object("RPG::BaseItem::Feature", 3);
    w.sym("@code");
    w.int(21);
    w.sym("@data_id");
    w.int(2);
    w.sym("@value");
    w.bytes.push(b'f');
    w.blob(b"1.0");
    w.register();
    w.sym("@opaque");
    w.bytes.push(b'u');
    w.sym("Table");
    w.blob(&[0, 255, 1, 128]);
    w.register();
    w.bytes
}

#[test]
fn ace_feature_float_survives_injection_and_encoding_ivars() {
    for encoding in [b'T', b'F'] {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("Data");
        fs::create_dir(&data).unwrap();
        let file = data.join("Actors.rvdata2");
        fs::write(&file, database("Hero", encoding)).unwrap();
        let plugin = RpgMakerVxaPlugin::new();
        let mut rows = plugin.extract(dir.path()).unwrap();
        assert_eq!(rows.len(), 1);
        rows[0].translation = Some("ES Hero".into());
        let report = plugin.inject(dir.path(), &rows).unwrap();
        assert_eq!(report.strings_written, 1);
        assert_eq!(report.strings_skipped, 0);
        assert_eq!(fs::read(file).unwrap(), database("ES Hero", encoding));
    }
}

fn map(lines: &[&str]) -> Vec<u8> {
    let mut w = Wire::new();
    w.object("RPG::Map", 1);
    w.sym("@events");
    w.bytes.push(b'{');
    w.packed(1);
    w.register();
    w.int(1);
    w.object("RPG::Event", 1);
    w.sym("@pages");
    w.array(1);
    w.object("RPG::Event::Page", 1);
    w.sym("@list");
    w.array(lines.len() + 3);
    for (i, line) in lines.iter().enumerate() {
        w.command(if i == 0 { 101 } else { 401 }, 1);
        w.string(line);
    }
    w.command(209, 2);
    w.int(0);
    w.object("RPG::MoveRoute", 1);
    w.sym("@list");
    w.array(1);
    let movement = w.object("RPG::MoveCommand", 2);
    w.sym("@code");
    w.int(45);
    w.sym("@parameters");
    w.array(1);
    let text = w.string("shared script");
    w.command(509, 1);
    w.link(movement);
    w.command(355, 1);
    w.link(text);
    w.bytes
}

#[test]
fn xp_move_route_and_string_links_after_growing_and_shrinking_messages() {
    for (original, translation, expected) in [
        (vec!["Hello"], "one two three four five six seven eight nine ten eleven twelve", vec!["one two three four five six seven eight", "nine ten eleven twelve"]),
        (vec!["Hello", "second"], "one two three four five six seven eight nine ten eleven twelve one two three four five six seven eight nine ten eleven twelve", vec!["one two three four five six seven eight", "nine ten eleven twelve one two three", "four five six seven eight nine ten", "eleven twelve"]),
        (vec!["Hello", "second line", "third line"], "short", vec!["short"]),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("Data"); fs::create_dir(&data).unwrap();
        let file = data.join("Map001.rxdata"); fs::write(&file, map(&original)).unwrap();
        let plugin = RpgMakerVxaPlugin::new();
        let mut rows = plugin.extract(dir.path()).unwrap(); assert_eq!(rows.len(), 1);
        rows[0].translation = Some(translation.into());
        let report = plugin.inject(dir.path(), &rows).unwrap();
        assert_eq!(report.strings_written, 1);
        assert_eq!(fs::read(file).unwrap(), map(&expected), "209/509 identity and string sharing must follow the new object indices");
    }
}

fn saved_continuation(remove: bool) -> Vec<u8> {
    let mut w = Wire::new();
    w.array(2);
    w.bytes.push(b'0');
    w.object("RPG::CommonEvent", 4);
    w.sym("@list");
    w.array(if remove { 2 } else { 3 });
    w.command(101, 1);
    let anchor_text = w.string(if remove { "short" } else { "Hello" });
    let mut continuation = 0;
    let mut text = 0;
    if !remove {
        continuation = w.objects;
        w.command(401, 1);
        text = w.string("kept by link");
    }
    w.command(0, 0);
    w.sym("@saved_command");
    if remove {
        w.command(401, 1);
        text = w.string("kept by link");
    } else {
        w.link(continuation);
    }
    w.sym("@saved_text");
    w.link(text);
    w.sym("@anchor_text");
    w.link(anchor_text);
    w.bytes
}

#[test]
fn surviving_links_rehome_deleted_commands_and_keep_translated_string_identity() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("Data");
    fs::create_dir(&data).unwrap();
    let file = data.join("CommonEvents.rxdata");
    fs::write(&file, saved_continuation(false)).unwrap();
    let plugin = RpgMakerVxaPlugin::new();
    let mut rows = plugin.extract(dir.path()).unwrap();
    assert_eq!(rows.len(), 1);
    rows[0].translation = Some("short".into());
    assert_eq!(plugin.inject(dir.path(), &rows).unwrap().strings_written, 1);
    assert_eq!(fs::read(file).unwrap(), saved_continuation(true));
}

#[test]
fn vx_and_ace_new_continuations_inherit_encoding() {
    fn common(lines: &[&str]) -> Vec<u8> {
        let mut w = Wire::new();
        w.array(2);
        w.bytes.push(b'0');
        w.object("RPG::CommonEvent", 1);
        w.sym("@list");
        w.array(lines.len() + 2);
        w.command(101, 4);
        w.bytes.push(b'I');
        w.string("");
        w.packed(1);
        w.sym("E");
        w.bytes.push(b'T');
        w.int(0);
        w.int(0);
        w.int(2);
        for line in lines {
            w.command(401, 1);
            w.bytes.push(b'I');
            w.string(line);
            w.packed(1);
            w.sym("E");
            w.bytes.push(b'T');
        }
        w.command(0, 0);
        w.bytes
    }
    for extension in ["rvdata", "rvdata2"] {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("Data");
        fs::create_dir(&data).unwrap();
        let file = data.join(format!("CommonEvents.{extension}"));
        fs::write(&file, common(&["Hello"])).unwrap();
        let plugin = RpgMakerVxaPlugin::new();
        let mut rows = plugin.extract(dir.path()).unwrap();
        assert_eq!(rows.len(), 1);
        rows[0].translation =
            Some("one two three four five six seven eight nine ten eleven twelve".into());
        assert_eq!(plugin.inject(dir.path(), &rows).unwrap().strings_written, 1);
        assert_eq!(
            fs::read(file).unwrap(),
            common(&[
                "one two three four five six seven eight",
                "nine ten eleven twelve"
            ])
        );
    }
}

#[test]
fn unrepresentable_file_is_skipped_with_reason_and_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("Data");
    fs::create_dir(&data).unwrap();
    let file = data.join("Actors.rvdata2");
    let mut w = Wire::new();
    w.array(2);
    w.bytes.push(b'0');
    w.object("RPG::Actor", 2);
    w.sym("@name");
    w.string("Hero");
    w.sym("@unknown");
    w.bytes.extend_from_slice(b"?payload");
    fs::write(&file, &w.bytes).unwrap();
    let mut row = StringEntry::new("Actors.rvdata2#1#@name", "Hero", file.clone());
    row.translation = Some("ES Hero".into());
    let report = RpgMakerVxaPlugin::new().inject(dir.path(), &[row]).unwrap();
    assert_eq!(report.strings_written, 0);
    assert_eq!(report.strings_skipped, 1);
    assert_eq!(report.files_modified, 0);
    assert_eq!(report.skip_reasons.get("unsupported_marshal"), Some(&1));
    assert_eq!(fs::read(file).unwrap(), w.bytes);
}

#[test]
fn malformed_links_lengths_and_trailing_bytes_are_rejected() {
    for bytes in [
        &b"\x04\x08@\x00"[..],
        &b"\x04\x08;\x00"[..],
        &b"\x04\x08\"\xfa"[..],
        &b"\x04\x08[\xfa"[..],
        &b"\x04\x08Tgarbage"[..],
        &b"\x04\x08f\x08x"[..],
        &b"\x04\x08l?\x00"[..],
    ] {
        assert!(
            MarshalValue::parse(bytes).is_err(),
            "accepted invalid stream: {bytes:?}"
        );
    }
}

/// Read-only probe; prints HEAD-compatible JSON even when files differ.
/// MARSHAL_PROBE_DIR=<Data> cargo test -p locust-formats --test
/// rpgmaker_marshal_lossless directory_roundtrip_probe -- --ignored --nocapture
#[test]
#[ignore = "set MARSHAL_PROBE_DIR to a scratch copy of an RGSS Data directory"]
fn directory_roundtrip_probe() {
    let directory =
        PathBuf::from(std::env::var_os("MARSHAL_PROBE_DIR").expect("MARSHAL_PROBE_DIR"));
    let mut files = fs::read_dir(directory)
        .unwrap()
        .map(|f| f.unwrap().path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e == "rxdata" || e == "rvdata" || e == "rvdata2")
        })
        .collect::<Vec<_>>();
    files.sort();
    let mut identical = 0;
    let mut errors = Vec::new();
    let mut different = Vec::new();
    let mut total_bytes = 0u64;
    for file in &files {
        let bytes = fs::read(file).unwrap();
        total_bytes += bytes.len() as u64;
        match MarshalValue::parse(&bytes) {
            Ok(value) => {
                if value.serialize() == bytes {
                    identical += 1;
                } else {
                    let output = value.serialize();
                    if different.len() < 3 {
                        let offset = bytes
                            .iter()
                            .zip(&output)
                            .position(|(a, b)| a != b)
                            .unwrap_or(bytes.len().min(output.len()));
                        println!(
                            "first difference {} at {}: input {:?}, output {:?} (lengths {}, {})",
                            file.display(),
                            offset,
                            &bytes[offset.saturating_sub(8)..(offset + 20).min(bytes.len())],
                            &output[offset.saturating_sub(8)..(offset + 20).min(output.len())],
                            bytes.len(),
                            output.len()
                        );
                    }
                    different.push(file.file_name().unwrap().to_string_lossy().into_owned());
                }
            }
            Err(error) => errors.push(format!("{}: {error}", file.display())),
        }
    }
    println!(
        "{}",
        serde_json::json!({"files":files.len(), "identical":identical,
        "different":different.len(), "different_examples":different.iter().take(12).collect::<Vec<_>>(),
        "bytes":total_bytes, "errors":errors})
    );
    if std::env::var_os("MARSHAL_PROBE_REQUIRE_IDENTICAL").is_some() {
        assert_eq!(identical, files.len());
    }
}
