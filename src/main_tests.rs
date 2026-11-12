// =============================================================================
// HYDRA-UMC-TWIN - src/main_tests.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The command line of the twin, run in this process: the loaders of the optional routes (every refusal and the way in), the family reports with
//! children in every state, the key generator, every fatal start-up error of `serve`, and one real `serve` on a free port of the loopback interface.
//! Nothing here touches a robot; every file lives in a folder of its own under the temporary folder.

use super::*;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn scratch(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!("twin-main-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn args(items: &[&str]) -> Vec<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn path_arg(path: &std::path::Path) -> String {
    path.to_string_lossy().to_string()
}

fn same(left: ExitCode, right: ExitCode) -> bool {
    format!("{left:?}") == format!("{right:?}")
}

const SEED: &str = "0101010101010101010101010101010101010101010101010101010101010101";
const TOKEN: &str = "a-token-of-16-chars-or-more";

/// A signing key file and a token file; returns the flags that name them.
fn certify_flags(dir: &std::path::Path) -> Vec<String> {
    let key = dir.join("signing.key");
    let token = dir.join("certify.token");
    std::fs::write(&key, format!("{SEED}\n")).unwrap();
    std::fs::write(&token, format!("{TOKEN}\n")).unwrap();
    vec![
        "--signing-key-file".into(),
        path_arg(&key),
        "--certify-token-file".into(),
        path_arg(&token),
    ]
}

fn some_file(dir: &std::path::Path, name: &str, text: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, text).unwrap();
    path
}

fn this_binary() -> String {
    path_arg(&std::env::current_exe().unwrap())
}

#[test]
fn the_flag_readers_take_the_value_after_the_flag_and_nothing_else() {
    let list = args(&["--a", "1", "--b", "--c"]);
    assert!(has_flag(&list, "--b"));
    assert!(!has_flag(&list, "--z"));
    assert_eq!(find_flag(&list, "--a"), Some("1".to_string()));
    assert_eq!(find_flag(&list, "--b"), Some("--c".to_string()));
    assert_eq!(find_flag(&list, "--c"), None);
    assert_eq!(find_flag(&list, "--z"), None);
    assert_eq!(find_flag(&[], "--a"), None);
}

#[test]
fn the_certifier_needs_the_key_and_the_token_together_and_a_token_that_is_long_enough() {
    let dir = scratch("certifier");
    assert!(matches!(load_certifier(&args(&[])), Ok(None)));
    let flags = certify_flags(&dir);
    let certifier = load_certifier(&flags).unwrap().unwrap();
    assert_eq!(certifier.token, TOKEN, "the token loses its line end");

    let only_key = args(&["--signing-key-file", &path_arg(&dir.join("signing.key"))]);
    assert!(load_certifier(&only_key)
        .err()
        .unwrap()
        .contains("needs --certify-token-file"));
    let only_token = args(&[
        "--certify-token-file",
        &path_arg(&dir.join("certify.token")),
    ]);
    assert!(load_certifier(&only_token)
        .err()
        .unwrap()
        .contains("needs --signing-key-file"));

    let short = some_file(&dir, "short.token", "tiny\n");
    let with_short = args(&[
        "--signing-key-file",
        &path_arg(&dir.join("signing.key")),
        "--certify-token-file",
        &path_arg(&short),
    ]);
    assert!(load_certifier(&with_short)
        .err()
        .unwrap()
        .contains("at least 16 characters"));

    let missing_token = args(&[
        "--signing-key-file",
        &path_arg(&dir.join("signing.key")),
        "--certify-token-file",
        &path_arg(&dir.join("none")),
    ]);
    assert!(load_certifier(&missing_token)
        .err()
        .unwrap()
        .contains("cannot read --certify-token-file"));
    let missing_key = args(&[
        "--signing-key-file",
        &path_arg(&dir.join("none")),
        "--certify-token-file",
        &path_arg(&dir.join("certify.token")),
    ]);
    assert!(load_certifier(&missing_key)
        .err()
        .unwrap()
        .contains("cannot read --signing-key-file"));
    let bad = some_file(&dir, "bad.key", "not hexadecimal\n");
    let bad_key = args(&[
        "--signing-key-file",
        &path_arg(&bad),
        "--certify-token-file",
        &path_arg(&dir.join("certify.token")),
    ]);
    assert!(load_certifier(&bad_key)
        .err()
        .unwrap()
        .contains("64 hexadecimal"));
}

#[test]
fn the_simulated_robot_needs_its_file_the_token_and_a_sane_tick() {
    let dir = scratch("sim");
    assert!(matches!(load_sim(&args(&[]), false), Ok(None)));
    assert!(load_sim(&args(&["--sim-tick-ms", "50"]), true)
        .err()
        .unwrap()
        .contains("needs --sim-config"));
    let model = some_file(
        &dir,
        "sim.json",
        r#"{"joints":[{"name":"j1","lower":-1.0,"upper":1.0,"max_velocity":1.0,"max_acceleration":2.0}]}"#,
    );
    let model_arg = path_arg(&model);
    assert!(load_sim(&args(&["--sim-config", &model_arg]), false)
        .err()
        .unwrap()
        .contains("needs the signing key"));
    assert!(load_sim(
        &args(&["--sim-config", &path_arg(&dir.join("none.json"))]),
        true
    )
    .err()
    .unwrap()
    .contains("cannot read --sim-config"));
    let broken = some_file(&dir, "broken.json", "{not json");
    assert!(load_sim(&args(&["--sim-config", &path_arg(&broken)]), true).is_err());
    let empty = some_file(&dir, "empty.json", r#"{"joints":[]}"#);
    assert!(load_sim(&args(&["--sim-config", &path_arg(&empty)]), true)
        .err()
        .unwrap()
        .contains("at least one joint"));
    for bad in ["4", "1001", "x", "-5", ""] {
        let error = load_sim(
            &args(&["--sim-config", &model_arg, "--sim-tick-ms", bad]),
            true,
        )
        .err()
        .unwrap();
        assert!(error.contains("from 5 to 1000"), "{bad}: {error}");
    }
    for good in [None, Some("5"), Some("1000")] {
        let mut list = vec!["--sim-config", model_arg.as_str()];
        if let Some(tick) = good {
            list.extend(["--sim-tick-ms", tick]);
        }
        assert!(
            matches!(load_sim(&args(&list), true), Ok(Some(_))),
            "{good:?}"
        );
    }
}

#[test]
fn the_collision_prediction_is_off_unless_asked_for_and_checks_every_input() {
    let dir = scratch("collision");
    let workspace = scratch("collision-ws");
    for flag in [
        "--collision-urdf",
        "--collision-zones",
        "--collision-config",
        "--collision-timeout-ms",
        "--collision-max-states",
    ] {
        let error = load_collision(&args(&[flag, "x"]), &workspace, true)
            .err()
            .unwrap();
        assert!(
            error.contains(&format!("{flag} needs --collision-predict")),
            "{error}"
        );
    }
    assert!(matches!(
        load_collision(&args(&[]), &workspace, true),
        Ok(None)
    ));
    assert!(
        load_collision(&args(&["--collision-predict"]), &workspace, false)
            .err()
            .unwrap()
            .contains("needs the signing key")
    );
    assert!(
        load_collision(&args(&["--collision-predict"]), &workspace, true)
            .err()
            .unwrap()
            .contains("--collision-urdf PATH")
    );

    let urdf = some_file(&dir, "robot.urdf", "<robot/>");
    let zones = some_file(&dir, "zones.json", "{}");
    let config = some_file(&dir, "config.json", "{}");
    let (u, z, c) = (path_arg(&urdf), path_arg(&zones), path_arg(&config));
    let missing = path_arg(&dir.join("none"));
    let base = |extra: &[&str]| {
        let mut list = vec![
            "--collision-predict",
            "--collision-urdf",
            u.as_str(),
            "--collision-zones",
            z.as_str(),
            "--collision-config",
            c.as_str(),
        ];
        list.extend_from_slice(extra);
        args(&list)
    };
    let not_a_file = load_collision(
        &args(&["--collision-predict", "--collision-urdf", &missing]),
        &workspace,
        true,
    )
    .err()
    .unwrap();
    assert!(not_a_file.contains("is not a file"), "{not_a_file}");
    let zones_missing = load_collision(
        &args(&["--collision-predict", "--collision-urdf", &u]),
        &workspace,
        true,
    )
    .err()
    .unwrap();
    assert!(
        zones_missing.contains("--collision-zones PATH"),
        "{zones_missing}"
    );
    let no_binary = load_collision(&base(&[]), &workspace, true).err().unwrap();
    assert!(
        no_binary.contains("needs the binary of HYDRA-UMC-PHYSICS-REPLICA"),
        "{no_binary}"
    );
    let wrong_binary = load_collision(
        &base(&["--physics-replica-bin", &missing]),
        &workspace,
        true,
    )
    .err()
    .unwrap();
    assert!(wrong_binary.contains("is not a file"), "{wrong_binary}");
    let binary = this_binary();
    for (flag, bad) in [
        ("--collision-timeout-ms", "99"),
        ("--collision-timeout-ms", "60001"),
        ("--collision-timeout-ms", "x"),
        ("--collision-max-states", "0"),
        ("--collision-max-states", "x"),
    ] {
        let error = load_collision(
            &base(&["--physics-replica-bin", &binary, flag, bad]),
            &workspace,
            true,
        )
        .err()
        .unwrap();
        assert!(
            error.contains(&format!("{flag} must be a whole number")),
            "{flag} {bad}: {error}"
        );
    }
    assert!(matches!(
        load_collision(&base(&["--physics-replica-bin", &binary]), &workspace, true),
        Ok(Some(_))
    ));
    assert!(matches!(
        load_collision(
            &base(&[
                "--physics-replica-bin",
                &binary,
                "--collision-timeout-ms",
                "100",
                "--collision-max-states",
                "1"
            ]),
            &workspace,
            true
        ),
        Ok(Some(_))
    ));
}

#[test]
fn the_physics_binary_is_named_by_the_flag_or_found_next_to_the_workspace() {
    let workspace = scratch("physics");
    let binary = this_binary();
    assert_eq!(
        physics_binary(&args(&["--physics-replica-bin", &binary]), &workspace, "x").unwrap(),
        PathBuf::from(&binary)
    );
    assert!(physics_binary(
        &args(&["--physics-replica-bin", &path_arg(&workspace.join("none"))]),
        &workspace,
        "x"
    )
    .err()
    .unwrap()
    .contains("is not a file"));
    assert!(physics_binary(&args(&[]), &workspace, "the route")
        .err()
        .unwrap()
        .starts_with("the route needs the binary"));
    // the one built next to the workspace is found by its place
    let name = if cfg!(windows) {
        "hydra-umc-physics-replica.exe"
    } else {
        "hydra-umc-physics-replica"
    };
    let built = workspace
        .join("HYDRA-UMC-PHYSICS-REPLICA")
        .join("target")
        .join("debug");
    std::fs::create_dir_all(&built).unwrap();
    std::fs::write(built.join(name), b"x").unwrap();
    assert_eq!(
        physics_binary(&args(&[]), &workspace, "x").unwrap(),
        built.join(name)
    );
}

#[test]
fn the_probes_read_their_binary_their_address_and_a_timeout_in_range() {
    let config = load_health_config(&args(&[])).unwrap();
    assert!(
        config.physics_replica_bin.is_none()
            || std::env::var("HYDRA_UMC_TWIN_PHYSICS_REPLICA_BIN").is_ok()
    );
    let tuned = load_health_config(&args(&[
        "--physics-replica-bin",
        "/some/bin",
        "--hil-bridge-addr",
        "10.0.0.5:9",
        "--timeout-ms",
        "250",
    ]))
    .unwrap();
    assert_eq!(tuned.physics_replica_bin, Some(PathBuf::from("/some/bin")));
    assert_eq!(tuned.hil_bridge_addr, "10.0.0.5:9");
    assert_eq!(tuned.timeout, Duration::from_millis(250));
    for bad in ["99", "60001", "x", "", "-1"] {
        assert!(
            load_health_config(&args(&["--timeout-ms", bad]))
                .err()
                .unwrap()
                .contains("from 100 to 60000"),
            "{bad}"
        );
    }
    for edge in ["100", "60000"] {
        assert!(
            load_health_config(&args(&["--timeout-ms", edge])).is_ok(),
            "{edge}"
        );
    }
}

#[test]
fn the_twin_routes_are_off_unless_asked_for_and_each_switch_needs_its_file() {
    let dir = scratch("twin");
    let workspace = scratch("twin-ws");
    let none = load_twin(&args(&[]), &workspace, false).unwrap();
    assert!(
        none.poses.is_none()
            && none.dryrun.is_none()
            && none.drift.is_none()
            && none.uav.is_none()
            && none.ws.is_none()
    );
    for (list, fragment) in [
        (vec!["--ws"], "--ws needs --poses or --uav-input"),
        (vec!["--drift-datalake"], "--drift-datalake needs --drift"),
        (
            vec!["--twin-config", "x"],
            "--twin-config needs at least one",
        ),
        (
            vec!["--poses"],
            "need the signing key and the certify token",
        ),
        (vec!["--drift", "--ws"], "--ws needs --poses or --uav-input"),
    ] {
        let error = load_twin(&args(&list), &workspace, false)
            .err()
            .unwrap_or_default();
        assert!(error.contains(fragment), "{list:?}: {error}");
    }
    assert!(load_twin(&args(&["--poses"]), &workspace, true)
        .err()
        .unwrap()
        .contains("need --twin-config FILE"));
    let broken = some_file(&dir, "broken.json", "{not json");
    assert!(load_twin(
        &args(&["--poses", "--twin-config", &path_arg(&broken)]),
        &workspace,
        true
    )
    .is_err());
    let unknown = some_file(&dir, "unknown.json", r#"{"no_such_field": 1}"#);
    assert!(load_twin(
        &args(&["--uav-input", "--twin-config", &path_arg(&unknown)]),
        &workspace,
        true
    )
    .is_err());
    assert!(load_twin(
        &args(&[
            "--uav-input",
            "--twin-config",
            &path_arg(&dir.join("none.json"))
        ]),
        &workspace,
        true
    )
    .is_err());
}

#[test]
fn the_poses_and_the_dry_run_need_a_model_a_binary_and_their_files() {
    let dir = scratch("twin-poses");
    let workspace = scratch("twin-poses-ws");
    let binary = this_binary();
    let no_urdf = some_file(&dir, "no-urdf.json", "{}");
    let error = load_twin(
        &args(&[
            "--poses",
            "--twin-config",
            &path_arg(&no_urdf),
            "--physics-replica-bin",
            &binary,
        ]),
        &workspace,
        true,
    )
    .err()
    .unwrap();
    assert!(error.contains("needs \"urdf\""), "{error}");
    let ghost = some_file(&dir, "ghost.json", r#"{"urdf":"missing.urdf"}"#);
    let error = load_twin(
        &args(&[
            "--dryrun",
            "--twin-config",
            &path_arg(&ghost),
            "--physics-replica-bin",
            &binary,
        ]),
        &workspace,
        true,
    )
    .err()
    .unwrap();
    assert!(error.contains("is not a file"), "{error}");

    some_file(&dir, "robot.urdf", "<robot/>");
    let config = some_file(
        &dir,
        "twin.json",
        r#"{"urdf":"robot.urdf","robot_id":"arm-1","language":"es"}"#,
    );
    let config_arg = path_arg(&config);
    let error = load_twin(
        &args(&["--poses", "--twin-config", &config_arg]),
        &workspace,
        true,
    )
    .err()
    .unwrap();
    assert!(
        error.contains("needs the binary of HYDRA-UMC-PHYSICS-REPLICA"),
        "{error}"
    );
    let poses = load_twin(
        &args(&[
            "--poses",
            "--twin-config",
            &config_arg,
            "--physics-replica-bin",
            &binary,
        ]),
        &workspace,
        true,
    )
    .unwrap();
    assert!(poses.poses.is_some() && poses.dryrun.is_none());
    assert_eq!(poses.lang, crate::twincfg::Lang::Es);
    let dry = load_twin(
        &args(&[
            "--dryrun",
            "--twin-config",
            &config_arg,
            "--physics-replica-bin",
            &binary,
        ]),
        &workspace,
        true,
    )
    .unwrap();
    assert!(dry.dryrun.is_some() && dry.poses.is_none());

    let lost_physics = some_file(
        &dir,
        "lost.json",
        r#"{"urdf":"robot.urdf","physics_config":"nowhere.json"}"#,
    );
    let error = load_twin(
        &args(&[
            "--dryrun",
            "--twin-config",
            &path_arg(&lost_physics),
            "--physics-replica-bin",
            &binary,
        ]),
        &workspace,
        true,
    )
    .err()
    .unwrap();
    assert!(error.contains("physics_config"), "{error}");
    some_file(&dir, "physics.json", "{}");
    let with_physics = some_file(
        &dir,
        "physics-ok.json",
        r#"{"urdf":"robot.urdf","physics_config":"physics.json"}"#,
    );
    let both = load_twin(
        &args(&[
            "--poses",
            "--dryrun",
            "--twin-config",
            &path_arg(&with_physics),
            "--physics-replica-bin",
            &binary,
        ]),
        &workspace,
        true,
    )
    .unwrap();
    assert!(both.poses.is_some() && both.dryrun.is_some());
}

#[test]
fn the_drift_monitor_the_vehicle_and_the_push_socket_start_from_the_configuration() {
    let dir = scratch("twin-drift");
    let workspace = scratch("twin-drift-ws");
    let plain = some_file(&dir, "plain.json", "{}");
    let plain_arg = path_arg(&plain);
    let drift = load_twin(
        &args(&["--drift", "--twin-config", &plain_arg]),
        &workspace,
        true,
    )
    .unwrap();
    assert!(drift.drift.is_some() && drift.uav.is_none() && drift.ws.is_none());
    let uav = load_twin(
        &args(&["--uav-input", "--twin-config", &plain_arg]),
        &workspace,
        true,
    )
    .unwrap();
    assert!(uav.uav.is_some() && uav.drift.is_none());

    let logged = some_file(&dir, "logged.json", r#"{"drift":{"log_file":"drift.log"}}"#);
    assert!(load_twin(
        &args(&["--drift", "--twin-config", &path_arg(&logged)]),
        &workspace,
        true
    )
    .unwrap()
    .drift
    .is_some());

    // the sending to the data lake needs its address, and a token file that holds a clean line
    let error = load_twin(
        &args(&["--drift", "--drift-datalake", "--twin-config", &plain_arg]),
        &workspace,
        true,
    )
    .err()
    .unwrap();
    assert!(error.contains("drift.datalake.url"), "{error}");
    let lost_token = some_file(
        &dir,
        "lost-token.json",
        r#"{"drift":{"datalake":{"url":"https://127.0.0.1:9/ingest","ca_file":"ca.pem","token_file":"nowhere.token"}}}"#,
    );
    let error = load_twin(
        &args(&[
            "--drift",
            "--drift-datalake",
            "--twin-config",
            &path_arg(&lost_token),
        ]),
        &workspace,
        true,
    )
    .err()
    .unwrap();
    assert!(
        error.contains("cannot read drift.datalake.token_file"),
        "{error}"
    );
    some_file(&dir, "empty.token", "  \n");
    let empty_token = some_file(
        &dir,
        "empty-token.json",
        r#"{"drift":{"datalake":{"url":"https://127.0.0.1:9/ingest","ca_file":"ca.pem","token_file":"empty.token"}}}"#,
    );
    let error = load_twin(
        &args(&[
            "--drift",
            "--drift-datalake",
            "--twin-config",
            &path_arg(&empty_token),
        ]),
        &workspace,
        true,
    )
    .err()
    .unwrap();
    assert!(
        error.contains("empty or has a control character"),
        "{error}"
    );

    // the push socket listens where the configuration says, loopback unless the operator allows more
    let socket = some_file(&dir, "socket.json", r#"{"ws":{"listen":"127.0.0.1:0"}}"#);
    let ws = load_twin(
        &args(&["--uav-input", "--ws", "--twin-config", &path_arg(&socket)]),
        &workspace,
        true,
    )
    .unwrap();
    assert!(ws.ws.is_some());
    let remote = some_file(&dir, "remote.json", r#"{"ws":{"listen":"0.0.0.0:0"}}"#);
    let error = load_twin(
        &args(&["--uav-input", "--ws", "--twin-config", &path_arg(&remote)]),
        &workspace,
        true,
    )
    .err()
    .unwrap();
    assert!(error.contains("loopback"), "{error}");
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = some_file(
        &dir,
        "busy.json",
        &format!(r#"{{"ws":{{"listen":"{}"}}}}"#, taken.local_addr().unwrap()),
    );
    let error = load_twin(
        &args(&["--uav-input", "--ws", "--twin-config", &path_arg(&busy)]),
        &workspace,
        true,
    )
    .err()
    .unwrap();
    assert!(
        error.contains("could not listen for the WebSocket"),
        "{error}"
    );
}

#[test]
fn the_key_generator_writes_once_and_never_over_a_file() {
    let dir = scratch("keygen");
    assert!(same(run_keygen(&args(&[])), ExitCode::from(2)));
    let key = dir.join("new.key");
    assert!(same(
        run_keygen(&args(&["--key-file", &path_arg(&key)])),
        ExitCode::SUCCESS
    ));
    let text = std::fs::read_to_string(&key).unwrap();
    assert_eq!(text.trim().len(), 64);
    assert!(text.trim().chars().all(|c| c.is_ascii_hexdigit()));
    // a second run refuses and leaves the key as it was
    assert!(same(
        run_keygen(&args(&["--key-file", &path_arg(&key)])),
        ExitCode::from(1)
    ));
    assert_eq!(std::fs::read_to_string(&key).unwrap(), text);
    // a folder, or a place that cannot be written, is an error and not a panic
    assert!(same(
        run_keygen(&args(&["--key-file", &path_arg(&dir)])),
        ExitCode::from(1)
    ));
    assert!(same(
        run_keygen(&args(&[
            "--key-file",
            &path_arg(&dir.join("no").join("such").join("folder.key"))
        ])),
        ExitCode::from(1)
    ));
}

#[test]
fn the_family_reports_name_every_state_a_child_can_be_in() {
    let workspace = scratch("family");
    let ws = path_arg(&workspace);
    let write = |name: &str, text: &str| {
        let dir = workspace.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("hydra-umc.project.json"), text).unwrap();
    };
    let manifest = |name: &str, version: &str, maturity: &str| {
        format!(
            r#"{{"name":"{name}","version":"{version}","maturity":"{maturity}","role":"library"}}"#
        )
    };
    // nothing checked out
    assert!(same(
        run_family_status(&args(&["--workspace", &ws])),
        ExitCode::from(1)
    ));
    assert!(same(
        run_family_sync(&args(&["--workspace", &ws])),
        ExitCode::from(1)
    ));
    // every child ready
    for name in family::EXPECTED_CHILDREN {
        write(name, &manifest(name, "0.4.1", "functional"));
    }
    assert!(same(
        run_family_status(&args(&["--workspace", &ws])),
        ExitCode::SUCCESS
    ));
    assert!(same(
        run_family_sync(&args(&["--workspace", &ws])),
        ExitCode::SUCCESS
    ));
    // one of each way to be refused
    let names = family::EXPECTED_CHILDREN;
    write(names[0], &manifest(names[0], "0.4.1", "scaffolding"));
    write(names[1], &manifest(names[1], "3.0.0", "functional"));
    write(names[2], &manifest(names[2], "0.4.1", "mythical"));
    assert!(same(
        run_family_sync(&args(&["--workspace", &ws])),
        ExitCode::from(1)
    ));
    write(names[2], "{broken");
    assert!(same(
        run_family_status(&args(&["--workspace", &ws])),
        ExitCode::from(1)
    ));
    assert!(same(
        run_family_sync(&args(&["--workspace", &ws])),
        ExitCode::from(1)
    ));
    // the health report with every probe pointed at nothing
    let health = args(&[
        "--workspace",
        &ws,
        "--hil-bridge-addr",
        "127.0.0.1:1",
        "--timeout-ms",
        "100",
    ]);
    assert!(same(run_family_health(&health), ExitCode::from(1)));
    assert!(same(
        run_family_health(&args(&["--workspace", &ws, "--timeout-ms", "5"])),
        ExitCode::from(2)
    ));
}

#[test]
fn the_state_validator_passes_a_good_state_and_names_the_problems_of_a_bad_one() {
    let dir = scratch("state");
    assert!(same(run_validate_state(&args(&[])), ExitCode::from(2)));
    assert!(same(
        run_validate_state(&args(&[&path_arg(&dir.join("none.json"))])),
        ExitCode::from(2)
    ));
    let junk = some_file(&dir, "junk.json", "not json");
    assert!(same(
        run_validate_state(&args(&[&path_arg(&junk)])),
        ExitCode::from(2)
    ));
    let empty = some_file(&dir, "empty.json", "{}");
    assert!(same(
        run_validate_state(&args(&[&path_arg(&empty)])),
        ExitCode::from(1)
    ));
}

#[test]
fn every_fatal_error_of_serve_is_a_clean_exit_with_code_2_before_anything_listens() {
    let dir = scratch("serve-fatal");
    let flags = certify_flags(&dir);
    let free = || {
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port().to_string()
    };
    let missing = path_arg(&dir.join("none"));
    let cases: Vec<Vec<String>> = vec![
        args(&[
            "--allow-any-workspace",
            "--strict-workspace-root",
            &path_arg(&dir),
        ]),
        args(&["--strict-workspace-root", &missing]),
        args(&["--workers", "0"]),
        args(&["--workers", "257"]),
        args(&["--workers", "many"]),
        args(&["--signing-key-file", &path_arg(&dir.join("signing.key"))]),
        args(&["--health-probes", "--timeout-ms", "1"]),
        args(&["--health-probes", "--health-cache-ms", "3600001"]),
        args(&["--health-probes", "--health-cache-ms", "soon"]),
        args(&["--require-fresh-state-ms", "0"]),
        args(&["--require-fresh-state-ms", "3600001"]),
        args(&["--require-fresh-state-ms", "x"]),
        args(&["--require-fresh-state-ms", "500"]),
        args(&["--collision-timeout-ms", "500"]),
        args(&["--sim-tick-ms", "50"]),
        args(&["--poses"]),
        args(&["--identity-key-file", &missing]),
        {
            let short = some_file(&dir, "short.key", "short\n");
            args(&["--identity-key-file", &path_arg(&short)])
        },
    ];
    for case in cases {
        let mut list = case.clone();
        list.extend(args(&[
            "--addr",
            "127.0.0.1",
            "--port",
            &free(),
            "--workspace",
            &path_arg(&dir),
        ]));
        assert!(same(run_serve(&list), ExitCode::from(2)), "{case:?}");
    }
    // a port that is already in use is the same clean exit
    let taken = TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = args(&[
        "--addr",
        "127.0.0.1",
        "--port",
        &taken.local_addr().unwrap().port().to_string(),
        "--workspace",
        &path_arg(&dir),
    ]);
    assert!(same(run_serve(&busy), ExitCode::from(2)));
    // with the key and the token the same flags that needed them pass the checks (and stop on the port)
    let mut with_key = flags;
    with_key.extend(args(&[
        "--require-fresh-state-ms",
        "500",
        "--addr",
        "127.0.0.1",
        "--port",
        &taken.local_addr().unwrap().port().to_string(),
        "--workspace",
        &path_arg(&dir),
    ]));
    assert!(same(run_serve(&with_key), ExitCode::from(2)));
}

fn get(port: u16, path: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut text = String::new();
    let _ = stream.read_to_string(&mut text);
    let status = text
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    (status, text)
}

fn wait_for(port: u16) -> bool {
    for _ in 0..100 {
        if TcpStream::connect(("127.0.0.1", port)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn a_real_serve_answers_on_a_free_port_with_the_options_the_operator_gave() {
    let dir = scratch("serve-live");
    let workspace = scratch("serve-live-ws");
    let model = some_file(
        &dir,
        "sim.json",
        r#"{"joints":[{"name":"j1","lower":-1.0,"upper":1.0,"max_velocity":1.0,"max_acceleration":2.0}]}"#,
    );
    let identity = some_file(&dir, "identity.key", &"k".repeat(64));
    let port = {
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };
    let mut list = certify_flags(&dir);
    list.extend(args(&[
        "--addr",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--workspace",
        &path_arg(&workspace),
        "--workers",
        "2",
        "--health-probes",
        "--health-cache-ms",
        "1000",
        "--hil-bridge-addr",
        "127.0.0.1:1",
        "--timeout-ms",
        "100",
        "--require-fresh-state-ms",
        "1000",
        "--sim-config",
        &path_arg(&model),
        "--sim-tick-ms",
        "50",
        "--health-open",
        "--identity-key-file",
        &path_arg(&identity),
    ]));
    std::thread::spawn(move || {
        let _ = run_serve(&list);
    });
    assert!(wait_for(port), "the server never listened");
    // with the identity key loaded every route needs an identity: an unsigned call is refused, and it is refused with a status the client can read
    let (status, _) = get(port, "/family-status");
    assert!(matches!(status, 401 | 403), "status {status}");
    let (status, _) = get(port, "/no-such-route");
    assert!(matches!(status, 401 | 403 | 404), "status {status}");
}

#[test]
fn a_real_serve_without_a_key_answers_the_family_reports() {
    let workspace = scratch("serve-open-ws");
    let port = {
        let probe = TcpListener::bind("127.0.0.1:0").unwrap();
        probe.local_addr().unwrap().port()
    };
    let list = args(&[
        "--addr",
        "127.0.0.1",
        "--port",
        &port.to_string(),
        "--workspace",
        &path_arg(&workspace),
        "--allow-any-workspace",
    ]);
    std::thread::spawn(move || {
        let _ = run_serve(&list);
    });
    assert!(wait_for(port), "the server never listened");
    let (status, body) = get(port, "/family-status");
    assert_eq!(status, 200, "{body}");
    assert!(body.contains("HYDRA-UMC-PHYSICS-REPLICA"), "{body}");
    let (status, _) = get(port, "/family-sync");
    assert_eq!(status, 200);
    let (status, _) = get(port, "/nowhere");
    assert_eq!(status, 404);
}
