#![allow(clippy::new_without_default)]

#[allow(unused_imports)] // we use maplin in tests
#[macro_use] extern crate maplit;
#[macro_use] extern crate log;
#[macro_use] extern crate multimap;
#[macro_use] extern crate derive_more;
#[macro_use] extern crate git_version;

// const GIT_VERSION : &str = git_describe!();
const GIT_VERSION : &str = git_version!();

use ascii::IntoAsciiString;
use dotenv::dotenv;
use guard::Guard;
use players::Players;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use weaponforcer::WeaponEnforcer;
use std::{env::var, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use vips::Vips;

use battlefield_rcon::{bf4::Bf4Client, rcon::{self, RconConnectionInfo}};
use mapmanager::{MapManager, PopState};
use mapvote::{
    config::{MapVoteConfig, MapVoteConfigJson},
    Mapvote,
};

use crate::weaponforcer::WeaponEnforcerConfig;

pub mod guard;
// pub mod commands;
pub mod mapmanager;
pub mod mapvote;
pub mod vips;
pub mod players;
mod stv;
pub mod weaponforcer;
pub mod humanlang;
mod logging;

fn get_rcon_coninfo() -> rcon::RconResult<RconConnectionInfo> {
    let ip = var("BFOX_RCON_IP").unwrap_or_else(|_| "127.0.0.1".into());
    let port = var("BFOX_RCON_PORT")
        .unwrap_or_else(|_| "47200".into())
        .parse::<u16>()
        .unwrap();
    let password = var("BFOX_RCON_PASSWORD").unwrap_or_else(|_| "smurf".into());
    Ok(RconConnectionInfo {
        ip,
        port,
        password: password.into_ascii_string()?,
    })
}

#[derive(Debug)]
enum ConfigError {
    Serde(serde_yaml::Error),
    Io(std::io::Error),
}

async fn load_config<T: DeserializeOwned>(path: &str) -> Result<T, ConfigError> {
    info!("Loading {}", path);
    let mut file = tokio::fs::File::open(path).await.map_err(ConfigError::Io)?;
    let mut s = String::new();
    file.read_to_string(&mut s).await.map_err(ConfigError::Io)?;
    let t: T = serde_yaml::from_str(&s).map_err(ConfigError::Serde)?;
    Ok(t)
}

#[allow(dead_code)]
async fn save_config<T: Serialize>(path: &str, obj: &T) -> Result<(), ConfigError> {
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(ConfigError::Io)?;
    let s = serde_yaml::to_string(obj).map_err(ConfigError::Serde)?;
    file.write_all(s.as_bytes())
        .await
        .map_err(ConfigError::Io)?;

    Ok(())
}

/// Convenience thing for loading stuff from Json.
#[derive(Debug, Serialize, Deserialize)]
struct MapManagerConfig {
    pop_states: Vec<PopState>,

    vehicle_threshold: usize,
    leniency: usize,
}

#[allow(clippy::or_fun_call)]
#[tokio::main]
async fn main() -> rcon::RconResult<()> {
    dotenv().ok(); // load (additional) environment variables from `.env` file in working directory.
    logging::init_logging();

    info!("This is BattleFox {}", GIT_VERSION);

    let coninfo = get_rcon_coninfo()?;
    let players = Arc::new(Players::new());
    let vips = Arc::new(Vips::new());

    let weaponforcer_config : WeaponEnforcerConfig = load_config("configs/weaponforcer.yaml").await.unwrap();
    let weaponforcer = WeaponEnforcer::new(weaponforcer_config);

    // let commands = Arc::new(Commands::new());

    let mapman_config: MapManagerConfig = load_config("configs/mapman.yaml").await.unwrap();
    let mapman = Arc::new(MapManager::new(
        Guard::new(mapman_config.pop_states).expect("Failed to validate map manager config"),
        mapman_config.vehicle_threshold,
        mapman_config.leniency,
    ));

    let mapvote_config: MapVoteConfigJson = load_config("configs/mapvote.yaml").await.unwrap();
    let mapvote = Mapvote::new(
        mapman.clone(),
        vips.clone(),
        players.clone(),
        MapVoteConfig::from_json(mapvote_config),
    )
    .await;


    // connect
    info!(
        "Connecting to {}:{} with password ***...",
        coninfo.ip, coninfo.port
    );
    let bf4 = Bf4Client::connect((coninfo.ip, coninfo.port), coninfo.password).await.unwrap();
    trace!("Connected!");

    // for i in 0..10 {
    //     bf4.say(format!("{}", i).repeat(20), Visibility::All).await.unwrap();
    //     sleep(Duration::from_millis(2000)).await;
    // }

    // for map in battlefield_rcon::bf4::Map::all() {
    //     let mut msg = "\t".to_string();
    //     for minlen in 0..3 {
    //         msg += &format!("{}", map.tab4_prefixlen(minlen));
    //         // let upper = map.short()[..minlen].to_ascii_uppercase();
    //         // let lower = map.short()[minlen..].to_string();
    //         // msg += &format!("[\t{}{}\t]  ", upper, lower); // TODO: trim last \t of last chunk item
    //     }
    //     msg += "|";
    //     bf4.say(msg, battlefield_rcon::bf4::Visibility::All).await.unwrap();
    //     sleep(Duration::from_millis(150)).await;
    // }

    // start parts.
    let mut jhs = Vec::new();

    // let bf4clone = bf4.clone();
    // jhs.push(tokio::spawn(async move { commands.run(bf4clone).await }));

    let bf4clone = bf4.clone();
    jhs.push(tokio::spawn(async move { players.run(bf4clone).await }));

    let bf4clone = bf4.clone();
    jhs.push(tokio::spawn(async move { mapvote.run(bf4clone).await }));

    let bf4clone = bf4.clone();
    jhs.push(tokio::spawn(async move { mapman.run(bf4clone).await }));

    let bf4clone = bf4.clone();
    jhs.push(tokio::spawn(async move { weaponforcer.run(&bf4clone).await }));

    // Wait for all our spawned tasks to finish.
    // This'll happen at shutdown, or never, when you CTRL-C.
    for jh in jhs.drain(..) {
        jh.await.unwrap()?
    }

    trace!("Exiting gracefully :)");

    Ok(())
}
