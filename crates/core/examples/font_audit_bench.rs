//! Repeatable synthetic cmap-audit workload; no third-party font assets needed.
use locust_core::font_validation::FontValidator;
use std::time::Instant;

fn main() {
    let source = std::env::args()
        .nth(1)
        .expect("path to Locust synthetic-ascii.ttf");
    let single = std::fs::read(source).unwrap();
    let face_count = 4u32;
    let mut collection = b"ttcf\x00\x01\x00\x00".to_vec();
    collection.extend_from_slice(&face_count.to_be_bytes());
    let header_size = 12 + face_count as usize * 4;
    for index in 0..face_count {
        collection.extend_from_slice(
            &((header_size + index as usize * single.len()) as u32).to_be_bytes(),
        );
    }
    for _ in 0..face_count {
        let mut face = single.clone();
        let base = collection.len() as u32;
        let tables = u16::from_be_bytes(face[4..6].try_into().unwrap()) as usize;
        for index in 0..tables {
            let record = 12 + index * 16 + 8;
            let offset = u32::from_be_bytes(face[record..record + 4].try_into().unwrap());
            face[record..record + 4].copy_from_slice(&(offset + base).to_be_bytes());
        }
        collection.extend(face);
    }
    let temp = tempfile::tempdir().unwrap();
    for index in 0..3 {
        std::fs::write(temp.path().join(format!("font-{index}.ttc")), &collection).unwrap();
    }
    let seed = "Hello world! Français español 中文 漢字 العربية עברית ไทย \n";
    let corpus = seed.repeat(8 * 1024 * 1024 / seed.len());
    let mut times = Vec::new();
    let mut expected = None;
    for _ in 0..4 {
        let start = Instant::now();
        let audit = FontValidator::audit_game_fonts(temp.path(), &[&corpus]).unwrap();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(audit.fonts.len(), 12);
        assert!(audit.issues.is_empty());
        let counts = (
            audit.fonts[0].total_unique_chars,
            audit.fonts[0].missing_count,
        );
        assert!(expected.is_none_or(|value| value == counts));
        expected = Some(counts);
    }
    println!(
        "{}",
        serde_json::json!({"bytes":corpus.len(),"font_files":3,"faces_per_font":4,"repetitions":4,"milliseconds":times,"unique_and_missing":expected})
    );
}
