// =============================================================================
// HYDRA-UMC-TWIN - src/twincfg.rs
// Copyright (C) 2026 JuanenRac (Electro Hobby 3D) <electrohobby3d@gmail.com>
// GPL-3.0-or-later - see LICENSE
// =============================================================================
//! The configuration file of the kinematic twin (`serve --twin-config FILE`) and the sentences of its answers in eight languages.
//!
//! Every number that tunes the poses, the dry run of a trajectory and the drift monitor lives here and not in the code: the freshness of the
//! reports, the limits of a request, the thresholds of the discrepancy, the file of the log, the address of the data lake. A field the file
//! does not know is an error, so a typo never silently leaves a threshold at its default. The paths in the file are relative to the file.
//!
//! The sentences (`message` of an answer) exist in the eight languages of the project (en, es, de, fr, it, ja, zh and pt of Portugal), chosen
//! with `?lang=xx` or `Accept-Language`, or by `language` in the file. What a program reads stays English: `code`, `verdict`, `level`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    Es,
    De,
    Fr,
    It,
    Ja,
    Zh,
    Pt,
}

impl Lang {
    pub const ALL: [Lang; 8] = [
        Lang::En,
        Lang::Es,
        Lang::De,
        Lang::Fr,
        Lang::It,
        Lang::Ja,
        Lang::Zh,
        Lang::Pt,
    ];

    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Es => "es",
            Lang::De => "de",
            Lang::Fr => "fr",
            Lang::It => "it",
            Lang::Ja => "ja",
            Lang::Zh => "zh",
            Lang::Pt => "pt",
        }
    }

    /// `es`, `es-ES`, `pt_PT`... (the part before the first `-` or `_`, in any case).
    pub fn from_tag(tag: &str) -> Option<Lang> {
        let primary = tag
            .trim()
            .split(['-', '_'])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        Lang::ALL.into_iter().find(|l| l.code() == primary)
    }

    /// The first language of an `Accept-Language` header that the twin has.
    pub fn from_accept_language(header: &str) -> Option<Lang> {
        header
            .split(',')
            .filter_map(|part| Lang::from_tag(part.split(';').next().unwrap_or("")))
            .next()
    }
}

/// The fixed sentences of the new routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Say {
    VerdictPass,
    VerdictFail,
    VerdictIncomplete,
    PosesIncomplete,
    DriftOk,
    DriftWarn,
    DriftAlarm,
    WsSessionEnded,
    UavSimulatedRefused,
    UavStateStale,
}

impl Say {
    #[cfg(test)]
    pub const ALL: [Say; 10] = [
        Say::VerdictPass,
        Say::VerdictFail,
        Say::VerdictIncomplete,
        Say::PosesIncomplete,
        Say::DriftOk,
        Say::DriftWarn,
        Say::DriftAlarm,
        Say::WsSessionEnded,
        Say::UavSimulatedRefused,
        Say::UavStateStale,
    ];
}

/// The sentence in the language. Index order: en, es, de, fr, it, ja, zh, pt.
pub fn say(lang: Lang, what: Say) -> &'static str {
    let row: [&'static str; 8] = match what {
        Say::VerdictPass => [
            "The trajectory stays inside every limit and clear of every zone in the simulation.",
            "La trayectoria respeta todos los limites y no toca ninguna zona en la simulacion.",
            "Die Bahn bleibt in der Simulation innerhalb aller Grenzen und frei von allen Zonen.",
            "La trajectoire reste dans toutes les limites et loin de toutes les zones en simulation.",
            "La traiettoria resta entro tutti i limiti e lontana da ogni zona nella simulazione.",
            "シミュレーションでは、軌道はすべての制限内にあり、どのゾーンにも入りません。",
            "在仿真中，轨迹未超出任何限制，也未进入任何区域。",
            "A trajetória respeita todos os limites e não toca em nenhuma zona na simulação.",
        ],
        Say::VerdictFail => [
            "The trajectory breaks a limit or is predicted to enter a zone or touch the robot itself.",
            "La trayectoria rompe un limite o se predice que entra en una zona o toca al propio robot.",
            "Die Bahn verletzt eine Grenze oder dringt voraussichtlich in eine Zone ein oder trifft den Roboter selbst.",
            "La trajectoire dépasse une limite ou devrait entrer dans une zone ou toucher le robot lui-même.",
            "La traiettoria supera un limite o si prevede che entri in una zona o tocchi il robot stesso.",
            "軌道が制限を超えるか、ゾーンへの進入またはロボット自身との接触が予測されます。",
            "轨迹超出了限制，或预计会进入某个区域或碰到机器人自身。",
            "A trajetória viola um limite ou prevê-se que entre numa zona ou toque no próprio robô.",
        ],
        Say::VerdictIncomplete => [
            "Not every check could run, so the trajectory is not declared safe.",
            "No se pudo hacer toda la comprobacion, asi que la trayectoria no se declara segura.",
            "Nicht jede Prüfung konnte laufen, daher gilt die Bahn nicht als sicher.",
            "Toutes les vérifications n'ont pas pu être faites, la trajectoire n'est donc pas déclarée sûre.",
            "Non tutti i controlli sono stati eseguiti, quindi la traiettoria non è dichiarata sicura.",
            "一部の確認を実行できなかったため、軌道は安全とは宣言されません。",
            "并非所有检查都能运行，因此该轨迹不被判定为安全。",
            "Nem todas as verificações puderam ser feitas, pelo que a trajetória não é declarada segura.",
        ],
        Say::PosesIncomplete => [
            "The twin has no fresh report for every joint it needs, so no pose is computed.",
            "El gemelo no tiene un informe reciente de cada articulacion necesaria, asi que no se calcula ninguna pose.",
            "Der Zwilling hat nicht für jedes benötigte Gelenk einen aktuellen Bericht, daher wird keine Pose berechnet.",
            "Le jumeau n'a pas de rapport récent pour chaque articulation nécessaire, aucune pose n'est donc calculée.",
            "Il gemello non ha un rapporto recente per ogni giunto necessario, quindi non calcola nessuna posa.",
            "必要な全関節の最新の報告がないため、姿勢は計算されません。",
            "孪生体没有所需每个关节的最新报告，因此不计算任何位姿。",
            "O gémeo não tem um relatório recente de cada articulação necessária, pelo que não calcula nenhuma pose.",
        ],
        Say::DriftOk => [
            "The real joint and the simulated one agree.",
            "La articulacion real y la simulada coinciden.",
            "Das reale und das simulierte Gelenk stimmen überein.",
            "L'articulation réelle et l'articulation simulée concordent.",
            "Il giunto reale e quello simulato concordano.",
            "実機の関節とシミュレーションの関節が一致しています。",
            "真实关节与仿真关节一致。",
            "A articulação real e a simulada coincidem.",
        ],
        Say::DriftWarn => [
            "The real joint differs from the simulated one by more than the warning threshold.",
            "La articulacion real se aparta de la simulada mas que el umbral de aviso.",
            "Das reale Gelenk weicht vom simulierten um mehr als die Warnschwelle ab.",
            "L'articulation réelle s'écarte de la simulée au-delà du seuil d'avertissement.",
            "Il giunto reale si discosta da quello simulato oltre la soglia di avviso.",
            "実機の関節がシミュレーションと警告しきい値を超えて異なっています。",
            "真实关节与仿真关节的差异超过了警告阈值。",
            "A articulação real afasta-se da simulada mais do que o limiar de aviso.",
        ],
        Say::DriftAlarm => [
            "The real joint keeps differing from the simulated one past the alarm threshold: the calibration may have drifted.",
            "La articulacion real sigue apartandose de la simulada mas que el umbral de alarma: la calibracion puede haberse descalibrado.",
            "Das reale Gelenk weicht weiter über die Alarmschwelle vom simulierten ab: Die Kalibrierung könnte sich verstellt haben.",
            "L'articulation réelle continue de s'écarter de la simulée au-delà du seuil d'alarme : l'étalonnage a peut-être dérivé.",
            "Il giunto reale continua a discostarsi da quello simulato oltre la soglia di allarme: la calibrazione potrebbe essersi alterata.",
            "実機の関節がアラームしきい値を超えてシミュレーションとずれ続けています。キャリブレーションがずれた可能性があります。",
            "真实关节持续偏离仿真关节并超过报警阈值：校准可能已经漂移。",
            "A articulação real continua a afastar-se da simulada além do limiar de alarme: a calibração pode ter derivado.",
        ],
        Say::WsSessionEnded => [
            "The session reached its time limit and the twin closed it; connect again to go on.",
            "La sesion llego a su limite de tiempo y el gemelo la cerro; vuelve a conectar para seguir.",
            "Die Sitzung hat ihre Zeitgrenze erreicht und der Zwilling hat sie geschlossen; verbinden Sie sich erneut, um fortzufahren.",
            "La session a atteint sa limite de durée et le jumeau l'a fermée ; reconnectez-vous pour continuer.",
            "La sessione ha raggiunto il limite di tempo e il gemello l'ha chiusa; riconnettiti per continuare.",
            "セッションが時間制限に達したため、ツインが切断しました。続けるには再接続してください。",
            "会话已达到时间上限，孪生体已将其关闭；请重新连接以继续。",
            "A sessão atingiu o limite de tempo e o gémeo fechou-a; volte a ligar para continuar.",
        ],
        Say::UavSimulatedRefused => [
            "This twin does not accept simulated vehicle states; only states of a real vehicle.",
            "Este gemelo no acepta estados simulados de un vehiculo; solo estados de un vehiculo real.",
            "Dieser Zwilling nimmt keine simulierten Fahrzeugzustände an, nur Zustände eines echten Fahrzeugs.",
            "Ce jumeau n'accepte pas les états simulés d'un véhicule, seulement ceux d'un véhicule réel.",
            "Questo gemello non accetta stati simulati di un veicolo, solo stati di un veicolo reale.",
            "このツインは車両のシミュレーション状態を受け付けません。実車両の状態のみ受け付けます。",
            "此孪生体不接受车辆的仿真状态，只接受真实车辆的状态。",
            "Este gémeo não aceita estados simulados de um veículo, apenas estados de um veículo real.",
        ],
        Say::UavStateStale => [
            "The last state of this vehicle is too old to be trusted.",
            "El ultimo estado de este vehiculo es demasiado antiguo para fiarse de el.",
            "Der letzte Zustand dieses Fahrzeugs ist zu alt, um ihm zu vertrauen.",
            "Le dernier état de ce véhicule est trop ancien pour qu'on s'y fie.",
            "L'ultimo stato di questo veicolo è troppo vecchio per fidarsene.",
            "この車両の最新の状態は古すぎて信頼できません。",
            "该车辆的最新状态过旧，不可信。",
            "O último estado deste veículo é demasiado antigo para merecer confiança.",
        ],
    };
    let index = match lang {
        Lang::En => 0,
        Lang::Es => 1,
        Lang::De => 2,
        Lang::Fr => 3,
        Lang::It => 4,
        Lang::Ja => 5,
        Lang::Zh => 6,
        Lang::Pt => 7,
    };
    row[index]
}

// ---- the file ----

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct PosesCfg {
    /// The joints the twin needs to place the robot; empty: every joint that was reported.
    pub joints: Vec<String>,
    /// A report older than this (milliseconds since it arrived) does not count.
    pub max_age_ms: u64,
    pub timeout_ms: u64,
}

impl Default for PosesCfg {
    fn default() -> Self {
        Self {
            joints: Vec::new(),
            max_age_ms: 2_000,
            timeout_ms: 5_000,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DryRunCfg {
    pub timeout_ms: u64,
    pub max_samples: usize,
    /// How long a verdict is good for, from the moment it was issued.
    pub validity_ms: u64,
    /// With true (the default) a verdict is `pass` only if the collision prediction also ran and found nothing.
    pub require_collision_check: bool,
    /// With true (the default) a predicted `warning` level (a zone entered only inside the margin) is not a `pass`.
    pub fail_on_warning: bool,
}

impl Default for DryRunCfg {
    fn default() -> Self {
        Self {
            timeout_ms: 10_000,
            max_samples: 512,
            validity_ms: 5_000,
            require_collision_check: true,
            fail_on_warning: true,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Threshold {
    pub warn: f64,
    pub alarm: f64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DatalakeCfg {
    /// `http://host:port` or `https://host:port` of HYDRA-UMC-DATALAKE.
    pub url: String,
    pub source_id: String,
    pub kind: String,
    /// A file with the `ingest` token (never in this file); empty: no token is sent.
    pub token_file: String,
    /// Samples waiting to be sent; when it is full a new one is dropped and counted, never waited for.
    pub queue: usize,
    pub timeout_ms: u64,
    /// With false (the default) only a loopback address is accepted, because the token and the samples would travel in clear text.
    pub allow_remote: bool,
    /// With an `https://` address: the PEM file with the certificate (or the CA) that the data lake must present. Nothing else is trusted, so
    /// there is no system store and no way to skip the check; an `https://` address without this file is an error.
    pub ca_file: String,
    /// With an `https://` address: the name the certificate must be valid for; empty: the host of the address.
    pub server_name: String,
}

impl Default for DatalakeCfg {
    fn default() -> Self {
        Self {
            url: String::new(),
            source_id: "hydra-umc-twin".into(),
            kind: "twin_drift".into(),
            token_file: String::new(),
            queue: 256,
            timeout_ms: 2_000,
            allow_remote: false,
            ca_file: String::new(),
            server_name: String::new(),
        }
    }
}

/// The WebSocket the twin pushes its state through (`serve --ws`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct WsCfg {
    /// The address of its own listener (the HTTP server of the twin has no upgrade of its own), loopback by default.
    pub listen: String,
    /// Sessions at once; one more is refused with a 503.
    pub max_clients: usize,
    /// How often the state is pushed.
    pub interval_ms: u64,
    /// How often the twin sends a ping to see that the client is alive.
    pub ping_interval_ms: u64,
    /// A client that sends nothing at all (not even the pong of a ping) for this long is dropped.
    pub idle_timeout_ms: u64,
    /// The longest a session lasts; then the twin closes it and the client connects again (and so shows its token again).
    pub max_session_s: u64,
    /// How long a client has to send its whole opening request.
    pub handshake_timeout_ms: u64,
    /// The largest frame accepted from a client (a client has nothing to say but pongs and closes).
    pub max_frame_bytes: usize,
    /// The `Origin` values accepted. A request that carries an `Origin` (a browser) and is not in this list is refused; one that carries none is not a browser.
    pub allowed_origins: Vec<String>,
    /// With false (the default) only a loopback address may be listened on, because the token would travel in clear text.
    pub allow_remote: bool,
}

impl Default for WsCfg {
    fn default() -> Self {
        Self {
            listen: "127.0.0.1:8112".into(),
            max_clients: 4,
            interval_ms: 500,
            ping_interval_ms: 15_000,
            idle_timeout_ms: 45_000,
            max_session_s: 3_600,
            handshake_timeout_ms: 5_000,
            max_frame_bytes: 4_096,
            allowed_origins: Vec::new(),
            allow_remote: false,
        }
    }
}

/// The input for the state of a UAV (`serve --uav-input`).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct UavCfg {
    /// The pose that carries the vehicle in the TwinState it sends.
    pub pose_name: String,
    pub max_body_bytes: u64,
    /// Vehicles followed at once; the state of another one is refused.
    pub max_vehicles: usize,
    /// A state older than this (milliseconds since it arrived) is shown as stale.
    pub max_age_ms: u64,
    /// With false (the default) a state that says `simulated` is refused: the twin takes a simulated vehicle for a real one only on purpose.
    pub accept_simulated: bool,
    /// A position farther than this from the home point (metres on any axis) is refused as a mistake.
    pub max_distance_m: f64,
}

impl Default for UavCfg {
    fn default() -> Self {
        Self {
            pose_name: "vehicle".into(),
            max_body_bytes: 8_192,
            max_vehicles: 8,
            max_age_ms: 5_000,
            accept_simulated: false,
            max_distance_m: 100_000.0,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct DriftCfg {
    pub warn: f64,
    pub alarm: f64,
    /// An alarm needs this many consecutive observations above the alarm threshold (one noisy reading is only a warning).
    pub alarm_after: u32,
    /// The real position, when the request leaves it out, comes from the newest `measured` report if it is at most this old.
    pub max_age_ms: u64,
    pub per_joint: BTreeMap<String, Threshold>,
    /// A JSON Lines file with the observations that are not `ok` and the changes of level; empty: no file.
    pub log_file: String,
    pub max_log_bytes: u64,
    pub datalake: DatalakeCfg,
}

impl Default for DriftCfg {
    fn default() -> Self {
        Self {
            warn: 0.02,
            alarm: 0.05,
            alarm_after: 3,
            max_age_ms: 2_000,
            per_joint: BTreeMap::new(),
            log_file: String::new(),
            max_log_bytes: 5 * 1024 * 1024,
            datalake: DatalakeCfg::default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct TwinConfig {
    pub robot_id: String,
    pub urdf: String,
    /// The configuration file of `check-trajectory` of HYDRA-UMC-PHYSICS-REPLICA (`serve --config` there); empty: its defaults.
    pub physics_config: String,
    pub language: String,
    pub poses: PosesCfg,
    pub dryrun: DryRunCfg,
    pub drift: DriftCfg,
    pub ws: WsCfg,
    pub uav: UavCfg,
}

impl Default for TwinConfig {
    fn default() -> Self {
        Self {
            robot_id: "robot".into(),
            urdf: String::new(),
            physics_config: String::new(),
            language: "en".into(),
            poses: PosesCfg::default(),
            dryrun: DryRunCfg::default(),
            drift: DriftCfg::default(),
            ws: WsCfg::default(),
            uav: UavCfg::default(),
        }
    }
}

fn in_range(name: &str, value: u64, low: u64, high: u64) -> Result<(), String> {
    if (low..=high).contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "{name} must be a whole number from {low} to {high}, got {value}"
        ))
    }
}

fn positive(name: &str, value: f64) -> Result<(), String> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(format!("{name} must be a number above zero"))
    }
}

impl TwinConfig {
    pub fn parse(text: &str) -> Result<TwinConfig, String> {
        let config: TwinConfig = serde_json::from_str(text)
            .map_err(|e| format!("the twin configuration is not valid: {e}"))?;
        config.validate()?;
        Ok(config)
    }

    pub fn load(path: &Path) -> Result<TwinConfig, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read --twin-config {}: {e}", path.display()))?;
        Self::parse(&text)
    }

    pub fn lang(&self) -> Lang {
        Lang::from_tag(&self.language).unwrap_or(Lang::En)
    }

    /// `path` of the file, resolved against the folder of the configuration file.
    pub fn resolve(base: &Path, value: &str) -> PathBuf {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            path
        } else {
            base.join(path)
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.robot_id.trim().is_empty()
            || self.robot_id.len() > 64
            || self.robot_id.chars().any(|c| c.is_control())
        {
            return Err("robot_id must have 1 to 64 characters and no control character".into());
        }
        if Lang::from_tag(&self.language).is_none() {
            return Err("language must be one of en, es, de, fr, it, ja, zh, pt".into());
        }
        let p = &self.poses;
        in_range("poses.max_age_ms", p.max_age_ms, 1, 3_600_000)?;
        in_range("poses.timeout_ms", p.timeout_ms, 100, 60_000)?;
        for joint in &p.joints {
            check_joint_name("poses.joints", joint)?;
        }
        let d = &self.dryrun;
        in_range("dryrun.timeout_ms", d.timeout_ms, 100, 120_000)?;
        in_range("dryrun.max_samples", d.max_samples as u64, 2, 4_096)?;
        in_range("dryrun.validity_ms", d.validity_ms, 100, 3_600_000)?;
        let r = &self.drift;
        positive("drift.warn", r.warn)?;
        positive("drift.alarm", r.alarm)?;
        if r.alarm < r.warn {
            return Err("drift.alarm must not be below drift.warn".into());
        }
        in_range("drift.alarm_after", r.alarm_after as u64, 1, 10_000)?;
        in_range("drift.max_age_ms", r.max_age_ms, 1, 3_600_000)?;
        in_range("drift.max_log_bytes", r.max_log_bytes, 1_024, 1 << 30)?;
        for (joint, t) in &r.per_joint {
            check_joint_name("drift.per_joint", joint)?;
            positive("drift.per_joint warn", t.warn)?;
            positive("drift.per_joint alarm", t.alarm)?;
            if t.alarm < t.warn {
                return Err(format!(
                    "drift.per_joint.{joint}: alarm must not be below warn"
                ));
            }
        }
        let l = &r.datalake;
        in_range("drift.datalake.queue", l.queue as u64, 1, 100_000)?;
        in_range("drift.datalake.timeout_ms", l.timeout_ms, 100, 60_000)?;
        if l.source_id.trim().is_empty() || l.kind.trim().is_empty() {
            return Err("drift.datalake.source_id and kind must not be empty".into());
        }
        if l.url.trim_start().starts_with("https://") && l.ca_file.trim().is_empty() {
            return Err("drift.datalake.url is https://: drift.datalake.ca_file must name the PEM file of the certificate to trust".into());
        }
        if l.server_name
            .chars()
            .any(|c| c.is_control() || c.is_whitespace())
        {
            return Err(
                "drift.datalake.server_name must not have spaces or control characters".into(),
            );
        }
        let w = &self.ws;
        if w.listen.trim().is_empty()
            || w.listen
                .chars()
                .any(|c| c.is_control() || c.is_whitespace())
        {
            return Err("ws.listen must be an address such as 127.0.0.1:8112".into());
        }
        in_range("ws.max_clients", w.max_clients as u64, 1, 256)?;
        in_range("ws.interval_ms", w.interval_ms, 50, 60_000)?;
        in_range("ws.ping_interval_ms", w.ping_interval_ms, 1_000, 600_000)?;
        in_range("ws.idle_timeout_ms", w.idle_timeout_ms, 2_000, 3_600_000)?;
        if w.idle_timeout_ms <= w.ping_interval_ms {
            return Err("ws.idle_timeout_ms must be above ws.ping_interval_ms, or a client that answers every ping would be dropped".into());
        }
        in_range("ws.max_session_s", w.max_session_s, 10, 604_800)?;
        in_range(
            "ws.handshake_timeout_ms",
            w.handshake_timeout_ms,
            100,
            60_000,
        )?;
        in_range("ws.max_frame_bytes", w.max_frame_bytes as u64, 125, 1 << 20)?;
        for origin in &w.allowed_origins {
            if origin.trim().is_empty()
                || origin.chars().any(|c| c.is_control() || c.is_whitespace())
            {
                return Err("ws.allowed_origins: an origin must be like https://studio.example, with no spaces".into());
            }
        }
        let u = &self.uav;
        if u.pose_name.trim().is_empty()
            || u.pose_name.len() > 64
            || u.pose_name.chars().any(|c| c.is_control())
        {
            return Err(
                "uav.pose_name must have 1 to 64 characters and no control character".into(),
            );
        }
        in_range("uav.max_body_bytes", u.max_body_bytes, 256, 1 << 20)?;
        in_range("uav.max_vehicles", u.max_vehicles as u64, 1, 1_024)?;
        in_range("uav.max_age_ms", u.max_age_ms, 1, 3_600_000)?;
        positive("uav.max_distance_m", u.max_distance_m)?;
        Ok(())
    }
}

/// A joint name goes into a command line (`--joints a=0.5,b=1`) and into a field name: no separator, no control character.
pub fn check_joint_name(what: &str, name: &str) -> Result<(), String> {
    if name.is_empty()
        || name.len() > crate::joint_state::MAX_JOINT_NAME_LEN
        || name
            .chars()
            .any(|c| c.is_control() || c == ',' || c == '=' || c.is_whitespace())
    {
        return Err(format!(
            "{what}: a joint name must have 1 to {} bytes and no comma, equals sign, space or control character: {name:?}",
            crate::joint_state::MAX_JOINT_NAME_LEN
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_valid_and_the_file_may_be_empty() {
        TwinConfig::default().validate().unwrap();
        let c = TwinConfig::parse("{}").unwrap();
        assert_eq!(c.drift.alarm_after, 3);
        assert!(c.dryrun.require_collision_check);
        assert!(!c.drift.datalake.allow_remote);
        assert!(!c.uav.accept_simulated);
        assert!(c.ws.allowed_origins.is_empty());
        assert!(c.ws.listen.starts_with("127.0.0.1:"));
    }

    #[test]
    fn an_https_address_is_valid_only_with_the_certificate_to_trust() {
        let ok =
            r#"{"drift":{"datalake":{"url":"https://lake.example:8443","ca_file":"lake-ca.pem"}}}"#;
        assert!(TwinConfig::parse(ok).is_ok());
    }

    #[test]
    fn a_field_the_file_does_not_know_or_a_wrong_value_is_an_error() {
        for bad in [
            r#"{"nope":1}"#,
            r#"{"drift":{"warm":1}}"#,
            r#"{"drift":{"warn":0}}"#,
            r#"{"drift":{"warn":0.5,"alarm":0.1}}"#,
            r#"{"drift":{"alarm_after":0}}"#,
            r#"{"drift":{"per_joint":{"a":{"warn":2,"alarm":1}}}}"#,
            r#"{"drift":{"per_joint":{"a=b":{"warn":1,"alarm":2}}}}"#,
            r#"{"poses":{"frame":"base"}}"#,
            r#"{"poses":{"joints":["a,b"]}}"#,
            r#"{"poses":{"max_age_ms":0}}"#,
            r#"{"dryrun":{"max_samples":1}}"#,
            r#"{"language":"xx"}"#,
            r#"{"robot_id":""}"#,
            r#"{"ws":{"port":1}}"#,
            r#"{"ws":{"max_clients":0}}"#,
            r#"{"ws":{"ping_interval_ms":30000,"idle_timeout_ms":30000}}"#,
            r#"{"ws":{"allowed_origins":["a b"]}}"#,
            r#"{"uav":{"max_vehicles":0}}"#,
            r#"{"uav":{"max_distance_m":-1}}"#,
            r#"{"uav":{"pose_name":""}}"#,
            r#"{"drift":{"datalake":{"url":"https://lake.example:8443"}}}"#,
        ] {
            assert!(TwinConfig::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn every_sentence_exists_in_the_eight_languages() {
        let mut seen = std::collections::BTreeSet::new();
        for what in Say::ALL {
            for lang in Lang::ALL {
                let text = say(lang, what);
                assert!(text.trim().len() > 5, "{lang:?} {what:?}");
                seen.insert((what as usize, text));
            }
            // no two languages share the same sentence
            let in_all: std::collections::BTreeSet<&str> =
                Lang::ALL.into_iter().map(|l| say(l, what)).collect();
            assert_eq!(in_all.len(), 8, "{what:?}");
        }
        assert_eq!(seen.len(), Say::ALL.len() * 8);
    }

    #[test]
    fn the_language_comes_from_a_tag_or_an_accept_language_header() {
        assert_eq!(Lang::from_tag("pt_PT"), Some(Lang::Pt));
        assert_eq!(Lang::from_tag("ZH-cn"), Some(Lang::Zh));
        assert_eq!(Lang::from_tag("xx"), None);
        assert_eq!(
            Lang::from_accept_language("ru;q=0.9, ja;q=0.8, en"),
            Some(Lang::Ja)
        );
        assert_eq!(Lang::from_accept_language("ru"), None);
    }
}
