//! Developer regression only: packetize existing private MF AUs, never a live media source.
use hub_encode_win::h264::EncodedVideoFrame;
use hub_webrtc::video::Packetizer;
use serde_json::Value;

#[test]
#[ignore = "requires explicit REMOTEHUB_MF_SAMPLE_DIR pointing to a private local MF sample"]
fn actual_mf_access_units_packetize_without_changing_sps() {
    let folder = std::path::PathBuf::from(
        std::env::var_os("REMOTEHUB_MF_SAMPLE_DIR").expect("private sample directory required"),
    );
    let report: Value =
        serde_json::from_slice(&std::fs::read(folder.join("encode-report.json")).unwrap()).unwrap();
    let data = std::fs::read(folder.join("desktop.h264")).unwrap();
    let mut offset = 0;
    let mut packetizer = Packetizer::default();
    let mut total_packets = 0;
    for frame in report["frames"].as_array().unwrap() {
        let bytes = frame["bytes"].as_u64().unwrap() as usize;
        let packets = packetizer
            .packets(
                EncodedVideoFrame {
                    annex_b: data[offset..offset + bytes].to_vec(),
                    pts_100ns: frame["pts_100ns"].as_i64().unwrap(),
                    duration_100ns: frame["duration_100ns"].as_i64().unwrap(),
                    nal_types: vec![],
                    is_idr: frame["idr"].as_bool().unwrap(),
                    encode_elapsed_ms: 0.0,
                },
                102,
            )
            .unwrap();
        assert!(!packets.is_empty());
        assert_eq!(packets.iter().filter(|p| p.header.marker).count(), 1);
        assert!(packets.iter().all(|p| p.payload.len() <= 1188));
        total_packets += packets.len();
        offset += bytes;
    }
    assert_eq!(offset, data.len());
    assert!(total_packets > report["frames"].as_array().unwrap().len());
    println!(
        "actual MF AUs={} RTP packets={} source SPS={} ignored_constraint_set5={}",
        report["frames"].as_array().unwrap().len(),
        total_packets,
        packetizer.source_sps_profile_level_id.as_deref().unwrap(),
        packetizer.ignored_constraint_set5
    );
}
