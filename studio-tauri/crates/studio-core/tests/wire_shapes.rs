//! TS-shaped JSON must deserialize into Command: the Tauri IPC path is the
//! only serde consumer, and the mock/tests bypass it.

use studio_core::Command;

#[test]
fn ts_camel_case_shapes_deserialize() {
    let id = "00000000-0000-0000-0000-000000000001";
    let cases = [
        (
            "setTerrainZScale",
            format!(r#"{{"type":"setTerrainZScale","id":"{id}","zScale":2.5}}"#),
        ),
        (
            "setAssetPath",
            format!(r#"{{"type":"setAssetPath","id":"{id}","path":"/a/b.png","byteSize":99}}"#),
        ),
        (
            "importGltf",
            r#"{"type":"importGltf","name":"duck.glb","path":"/m/duck.glb","kind":"glb","byteSize":4096,"node":null}"#
                .to_string(),
        ),
        (
            "setTerrainDem",
            format!(
                r#"{{"type":"setTerrainDem","id":"{id}","asset":"{id}","elevationRange":[1,2],"worldOriginXz":[0.0,0.0],"worldSpanXz":[4000.0,3800.0]}}"#
            ),
        ),
        (
            "setGltfNode",
            format!(r#"{{"type":"setGltfNode","id":"{id}","node":"Body"}}"#),
        ),
        (
            "setRenderSettings",
            r#"{"type":"setRenderSettings","render":{"width":1920,"height":1080,"exposure":1.35,"shadows":true,"sky":true,"iblIntensity":1.0,"stillAov":{"albedo":true,"normal":false,"depth":true},"stillDenoise":true}}"#
                .to_string(),
        ),
        (
            "setTerrainVector",
            format!(
                r#"{{"type":"setTerrainVector","id":"{id}","layer":{{"asset":"{id}","colour":[0.9,0.29,0.1,1.0],"widthMetres":40.0}}}}"#
            ),
        ),
        (
            "applyMapLook",
            format!(r#"{{"type":"applyMapLook","id":"{id}"}}"#),
        ),
        (
            "setTransform",
            format!(
                r#"{{"type":"setTransform","id":"{id}","transform":{{"translation":{{"x":0.0,"y":0.0,"z":0.0}},"rotationDeg":{{"x":0.0,"y":0.0,"z":0.0}},"scale":{{"x":1.0,"y":1.0,"z":1.0}}}}}}"#
            ),
        ),
        (
            "batch",
            format!(
                r#"{{"type":"batch","commands":[{{"type":"renameObject","id":"{id}","name":"X"}}]}}"#
            ),
        ),
    ];
    let mut failures = Vec::new();
    for (name, case) in cases {
        if let Err(error) =
            serde_json::from_str::<Command>(&case).map_err(|error| error.to_string())
        {
            failures.push(format!("{name}: {error}"));
        }
    }
    assert!(
        failures.is_empty(),
        "TS shapes failed:\n{}",
        failures.join("\n")
    );
}
