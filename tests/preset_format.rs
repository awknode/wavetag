const SAMPLE: &str = r#"
{
  "version": "0.6.0",
  "params": {
    "shape": {
      "i32": 16
    },
    "bend": {
      "f32": 0.5
    },
    "sync": {
      "bool": true
    },
    "rate": {
      "f32": 4.0
    },
    "hz": {
      "f32": 4.0
    },
    "depth": {
      "f32": 1.0
    },
    "smooth": {
      "f32": 5.0
    },
    "phase": {
      "f32": 0.0
    },
    "width": {
      "f32": 0.0
    },
    "retrig": {
      "bool": false
    },
    "zerox": {
      "bool": true
    },
    "output": {
      "f32": 1.0
    }
  },
  "fields": {}
}"#;
#[test]
fn factory_preset_parses() {
    let s: nih_plug::wrapper::state::PluginState = serde_json::from_str(SAMPLE).unwrap();
    assert_eq!(s.params.len(), 12);
    assert!(matches!(s.params["shape"], nih_plug::wrapper::state::ParamValue::I32(16)));
}
