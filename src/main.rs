use std::{cmp, fs};

use anyhow::{Context, Result};
use bi0s_adf::{
    api::ApiCommand, flagbot::TcpConfig, ADMIN_API_PORT, DEFAULT_NOFILE_LIMIT,
    FLAG_SUBMISSION_PORT,
};
use clap::{Arg, Command};
use log::{error, warn};
use rlimit::Resource;
use tracing::info;

fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    let matches = Command::new("bi0s-adf")
        .version("0.3")
        .author("folks @teambi0s")
        .about("Attack and Defence framework")
        .arg_required_else_help(true)
        .subcommand(
            Command::new("run")
                .about("Run the game")
                .arg(Arg::new("name").required(true).value_name("NAME"))
                .arg(
                    Arg::new("data_path")
                        .help(
                            "Directory to store all the logs and database",
                        )
                        .long("data_path")
                        .value_name("DATAPATH")
                        .default_value("data"),
                ),
        )
        .subcommand(
            Command::new("dump")
                .about("Dump all details from the database as JSON")
                .arg(Arg::new("db").required(true).value_name("DB").help("Path to the database"))
                .arg(Arg::new("output").required(true).value_name("OUTPUT").help("Directory to output all the json files"))
        )
        .subcommand(
            Command::new("command")
                .about("Send Commands to the Framework")
                .arg_required_else_help(true)
                .subcommand(Command::new("pause").about("Pause the Game"))
                .subcommand(Command::new("start").about("Start the Game"))
                .subcommand(Command::new("f_start").about("Start Flag Submission"))
                .subcommand(Command::new("f_pause").about("Pause Flag Submission"))
                .subcommand(
                    Command::new("f_throttle").about("Throttle flag submissions with the configured tcp_config")
                            .arg_required_else_help(true)
                            .arg(Arg::new("state")
                            .value_parser(clap::value_parser!(bool))
                            .help("Dictates whether to throttle flag submissions of players")))
                .subcommand(
                    Command::new("tcp_config")
                    .arg_required_else_help(true)
                    .about("Update the flag submission error limits")
                    .arg(
                        Arg::new("invalid_flag")
                        .long("invalid_flag")
                        .help("Number of Invalid Flags to receive before closing the connection.")
                        .value_parser(clap::value_parser!(u64))
                        .default_value("1000")
                        .value_name("INVALID_FLAG")
                    )
                    .arg(
                        Arg::new("expired_flag")
                        .long("expired_flag")
                        .help("Number of Expired Flags to receive before closing the connection.")
                        .default_value("1000")
                        .value_parser(clap::value_parser!(u64))
                        .value_name("EXPIRED_FLAG")
                    )
                    .arg(
                        Arg::new("duplicate_submission")
                        .long("duplicate_submission")
                        .help("Number of Duplicate Submissions to receive before closing the connection.")
                        .value_parser(clap::value_parser!(u64))
                        .default_value("1000")
                        .value_name("DUP_SUB")
                    )
                    .arg(
                        Arg::new("self_flag")
                        .long("self_flag")
                        .help("Number of Self Flags to receive before closing the connection.")
                        .value_parser(clap::value_parser!(u64))
                        .default_value("1000")
                        .value_name("SELF_FLAG")
                    )
                    .arg(
                        Arg::new("max_connection")
                        .long("max_connection")
                        .help("Number of Connections allowed for a team.")
                        .value_parser(clap::value_parser!(u64))
                        .default_value("50")
                        .value_name("MAX_CONN")
                    ),
                ),
        )
        .subcommand(
            Command::new("init")
                .about("Initialize new game")
                .arg_required_else_help(true)
                .arg(
                    Arg::new("name")
                        .help("Name of the contest")
                        .required(true)
                        .value_name("NAME"),
                )
                .arg(
                    Arg::new("team_list")
                        .help("Path to list of teams in json format")
                        .long("teams")
                        .value_name("TEAMLIST")
                        .required(true),
                )
                .arg(
                    Arg::new("service_list")
                        .help("Path to list of services in json format")
                        .long("services")
                        .value_name("SERVICELIST")
                        .required(true),
                )
                .arg(
                    Arg::new("flag_life")
                        .help(
                            "How many rounds should the flag be kept valid",
                        )
                        .long("flag_life")
                        .value_name("FLAGLIFE")
                        .default_value("30")
                        .value_parser(clap::value_parser!(usize)),
                )
                .arg(
                    Arg::new("total_round")
                        .help(
                            "Number of rounds in the game",
                        )
                        .long("total_round")
                        .value_name("TOTALROUND")
                        .default_value("300")
                        .value_parser(clap::value_parser!(usize)),
                )
                .arg(
                    Arg::new("data_path")
                        .help(
                            "Directory to store all the logs and database",
                        )
                        .long("data_path")
                        .value_name("DATAPATH")
                        .default_value("data"),
                )
                .arg(
                    Arg::new("tick_time")
                        .help("Duration of one round in seconds")
                        .long("tick_time")
                        .value_name("TICKTIME")
                        .default_value("120")
                        .value_parser(clap::value_parser!(usize)),
                ),
        )
        .get_matches();

    match matches.subcommand() {
        Some(("run", sub_matches)) => {
            let name = sub_matches
                .get_one::<String>("name")
                .context("Failed to parse name of the game from command line argument")?
                .to_owned();

            info!(message = "Running Game : ", %name);

            let data_path = sub_matches
                .get_one::<String>("data_path")
                .context("Failed to parse data directory path from command line arguments")?
                .to_owned();

            let mut path = std::path::PathBuf::from(data_path);
            path.push(&name);

            let db = bi0s_adf::db::Db::new(&path)
                .context("Opening sqlite database")?;

            let teams = bi0s_adf::db::tables::TeamList::all(&db)
                .context("Retrieving Teams List from database")?;

            let services = bi0s_adf::db::tables::ServiceList::all(&db)
                .context("Retrieving Service List from database")?;

            let config = bi0s_adf::db::tables::Config::get(&db)
                .context("Retrieving Config from database")?;

            let slots = bi0s_adf::db::tables::ServiceSlots::all(&db)
                .context("Retrieving Service Slots from database")?;

            bi0s_adf::config::log(&config);
            bi0s_adf::service::log(&services);
            bi0s_adf::teams::log(&teams);

            let game = bi0s_adf::game::Game::<
                bi0s_adf::flag::DefaultFlag,
                bi0s_adf::scoring::faust::FaustCTF,
            >::new(config, services, teams, slots, db)?;

            // Set proper open file limits
            set_limits()?;

            let rt = tokio::runtime::Runtime::new()?;
            let fut = async move {
                if let Err(err) = game.run().await {
                    error!("Game Returned : {}", err)
                }
            };

            rt.block_on(fut);
        }

        Some(("command", sub_matches)) => {
            let command = match sub_matches.subcommand() {
                Some(("pause", _)) => ApiCommand::Pause,
                Some(("start", _)) => ApiCommand::Start,
                Some(("f_start", _)) => ApiCommand::StartFlagSubmission,
                Some(("f_pause", _)) => ApiCommand::PauseFlagSubmission,
                Some(("f_throttle", sub_match)) => {
                    let state = sub_match
                        .get_one::<bool>("state")
                        .context("Unable to process state value")?
                        .to_owned();
                    ApiCommand::ThrottleSubmission(state)
                }
                Some(("tcp_config", sub_matches)) => {
                    let invalid_flag = sub_matches
                        .get_one::<u64>("invalid_flag")
                        .context("Unable to process invalid_flag value")?
                        .to_owned();
                    let expired_flag = sub_matches
                        .get_one::<u64>("expired_flag")
                        .context("Unable to process expired_flag value")?
                        .to_owned();

                    let duplicate_submission = sub_matches
                        .get_one::<u64>("duplicate_submission")
                        .context(
                            "Unable to process duplicate_submission value",
                        )?
                        .to_owned();
                    let self_flag = sub_matches
                        .get_one::<u64>("self_flag")
                        .context("Unable to process self_flag value")?
                        .to_owned();
                    let max_connection = sub_matches
                        .get_one::<u64>("max_connection")
                        .context("Unable to process max_connection value")?
                        .to_owned();

                    let tcp_config = TcpConfig {
                        non_buffering_limit: 100,
                        invalid_flags_limit: invalid_flag,
                        self_flags_limit: self_flag,
                        expired_flags_limit: expired_flag,
                        duplicate_submission_limit: duplicate_submission,
                        max_connection,
                    };
                    ApiCommand::SetTcpConfig(tcp_config)
                }
                _ => unreachable!(),
            };
            info!("Running Command: {:?}", &command);
            let _result = reqwest::blocking::Client::new()
                .post(format!("http://127.0.0.1:{}/admin", ADMIN_API_PORT))
                .json(&command)
                .send()?
                .text()?;
        }

        Some(("init", sub_matches)) => {
            info!("Initializing game");
            let name = sub_matches
                .get_one::<String>("name")
                .context("Extracting game name from command line arguments")?
                .to_owned();

            let data_path = sub_matches
                .get_one::<String>("data_path")
                .context("Retrieving data directory path from command line arguments")?
                .to_owned();

            let flag_life = sub_matches
                .get_one::<usize>("flag_life")
                .context("Parsing flag lifetime from command line arguments")?
                .to_owned() as u64;

            let tick_time = sub_matches
                .get_one::<usize>("tick_time")
                .context(
                    "Parsing tick time interval from command line arguments",
                )?
                .to_owned() as u64;

            let total_round = sub_matches
                .get_one::<usize>("total_round")
                .context("Parsing total number of rounds from command line arguments")?
                .to_owned() as u64;

            // Create data directory for storing all the configuration for resuming the game.
            let mut path = std::path::PathBuf::from(data_path);
            fs::create_dir_all(&path)
                .context("Creating directory for storing data")?;

            path.push(&name);

            if path.exists() {
                error!("Path to database already exists: {}", path.display());
                return Ok(());
            }

            // Create a new sqlite database
            let db = bi0s_adf::db::Db::new(&path)
                .context("Creating a new sqlite database")?;

            // Initialize all the required tables for the database.
            bi0s_adf::db::journal::initialize_ledger(&db)
                .context("Initializing sqlite database tables")?;

            bi0s_adf::db::tables::initialize_database(&db)
                .context("Initializing sqlite database tables")?;

            // Store the configuration of the game to the database.
            bi0s_adf::db::tables::Config::insert(
                &db,
                &name,
                flag_life,
                tick_time,
                FLAG_SUBMISSION_PORT,
                total_round,
            )
            .context("Populating Config table")?;

            let config = bi0s_adf::db::tables::Config::get(&db)?;
            bi0s_adf::config::log(&config);

            let service_list_file = sub_matches
                .get_one::<String>("service_list")
                .context("Parsing service_list file")?;

            let teams_list_file = sub_matches
                .get_one::<String>("team_list")
                .context("Parsing team_list file")?;

            // Make sure the service and teams file is of correct format and commit to database.
            let services =
                bi0s_adf::service::commit_to_database(service_list_file, &db)
                    .context(
                    "Validating and committing Service details to database",
                )?;

            let teams =
                bi0s_adf::teams::commit_to_database(teams_list_file, &db)
                    .context(
                        "Validating and committing Teams details to database",
                    )?;

            bi0s_adf::service::log(&services);
            bi0s_adf::teams::log(&teams);
        }

        Some(("dump", sub_matches)) => {
            let db_path =
                sub_matches.get_one::<String>("db").context("Unable to extract database path from command line arguments")?.to_owned();

            let output_dir = sub_matches
                .get_one::<String>("output")
                .context("Failed to parse output directory path from command line arguments")?
                .to_owned();

            let output_dir = std::path::PathBuf::from(output_dir);
            let db_path = std::path::PathBuf::from(db_path);

            let db = bi0s_adf::db::Db::new(&db_path)
                .context("Opening sqlite database")?;

            let dump = bi0s_adf::dump::Dump::new(db, output_dir);
            dump.dump_everything()?;
        }

        _ => unreachable!(),
    }
    Ok(())
}

fn set_limits() -> anyhow::Result<()> {
    // Set proper limits
    let (o_soft, hard) = Resource::NOFILE.get()?;
    let target = cmp::min(DEFAULT_NOFILE_LIMIT, hard);
    Resource::NOFILE.set(target, hard)?;
    let (soft, hard) = Resource::NOFILE.get()?;
    info!("Increased ulimit to: soft = {soft}, hard = {hard}");
    if o_soft == soft {
        warn!("Old soft limit and new soft limit are the same!!!");
    }

    if soft <= 1024 {
        warn!("Increase the ulimit for the number of open files this program can open");
    }

    Ok(())
}
