# Attack Defense Framework

## Server Setup
### Install Necessary Packages

The package names are specified for Debian 12.

- [rust](https://rustup.rs/)
- docker - `curl -fsSL https://get.docker.com -o get-docker.sh`
- build-essential, pkg-config, libssl-dev, protobuf-compiler, libprotobuf-dev, sqlite3, libsqlite3-dev

### Supported Commands
```
Attack and Defense Framework

Usage: bi0s-adf [COMMAND]

Commands:
  run      Run the game
  dump     Dump all details from the database as JSON
  command  Send commands to the Framework
  init     Initialize new game
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

### Configuring the Game

The details about the services are provided to the framework as a JSON file, e.g.:

```
[
    {
        // Name of the service.
        "name": "message_box",
        // Port on which the service will be exposed on the player VM.
        "port": 1337,
        // Number of flag slots available for the service.
        "no_flag_slot": 3,
        // Service's checker IP address
        "checker_host": "127.0.0.1",
        // Service's checker port
        "checker_port": 50051
    }
]
```

The details about the teams are also provided as a JSON file, e.g.:

```
[
  {"ip": "10.42.0.2", "name": "team-0"}, 
  {"ip": "10.42.0.3", "name": "team-1"}, 
  {"ip": "10.42.0.4", "name": "team-2"}
]
```

The file contains the name and IP address of each team.

Example files can be found in the `config/` directory.

### Running the Game

1. Initialize the game with necessary details. This is done by calling the `init` subcommand.

```
Initialize new game

Usage: bi0s-adf init [OPTIONS] --teams <TEAMLIST> --services <SERVICELIST> <NAME>

Arguments:
  <NAME>  Name of the contest

Options:
      --teams <TEAMLIST>          Path to list of teams in JSON format
      --services <SERVICELIST>    Path to list of services in JSON format
      --flag_life <FLAGLIFE>      How many rounds should the flag be kept valid [default: 30]
      --total_round <TOTALROUND>  Number of rounds in the game [default: 300]
      --data_path <DATAPATH>      Directory to store all the logs and database [default: data]
      --tick_time <TICKTIME>      Duration of one round in seconds [default: 120]
  -h, --help                      Print help
```

> cargo run --release -- init --teams config/teams.json  --services config/service.json --tick_time 10 --data_path <data_path>  <game_name>

2. Once the game is initialized, we can start the framework by running the `run` subcommand.

```
Run the game

Usage: bi0s-adf run [OPTIONS] <NAME>

Arguments:
  <NAME>

Options:
      --data_path <DATAPATH>  Directory to store all the logs and database [default: data]
  -h, --help                  Print help
```

> cargo run --release -- run <game_name> --data_path <data_path>

3. Once the framework has started, it can be controlled by sending admin commands using the `command` subcommand.

```
Send Commands to the Framework

Usage: bi0s-adf command [COMMAND]

Commands:
  pause       Pause the Game
  start       Start the Game
  f_start     Start Flag Submission
  f_pause     Pause Flag Submission
  tcp_config  Update the flag submission error limits
  help        Print this message or the help of the given subcommand(s)

Options:
  -h, --help  Print help
```

### Writing a Checker

The gRPC interface between the service checker and the framework is defined in `proto/checker.proto`. The challenge authors are responsible for writing the checker. This can be done by writing a gRPC server with the provided protocol definition. An example gRPC server can be found in the `checker-client` folder.

The challenge authors have to implement 3 RPC calls:

- PlantFlag: The framework uses this RPC call to plant a flag in the service.
- CheckFlag: The framework will provide a flag and its associated token and identifier. The checker has to verify if the player's service contains this specific flag and is accessible.
- CheckService: The framework uses this to check the functionality of the service.

Notes:

- Challenge authors must ensure the checker returns proper reason and status as return values.
- Make sure no internal errors are shown in the `reason` field of `ServiceStatus`, as this value is shown in the scoreboard.
- A single RPC call should not take more than 20% of the round duration to execute.
- Ensure the checker can handle checking all teams' services within the above-mentioned time duration.

### Testing

The example `checker-client` can be used as a service checker for testing. It will write all the flags in the following format: `<ip>:<flag>` to a file 
called `flags.txt`. Later, the `submit.sh` bash script can be used to submit the flags as different teams. The script sets up a network namespace for creating 
TCP connections from different team IP addresses.

> `cat checker-client/flags.txt | grep 10.42.0.4 | tail -n 1 | cut -d ":" -f 2 | ./submit.sh 10.42.0.2`

The above command retrieves the latest flag for the team with IP `10.42.0.4` and submits it as the team with IP `10.42.0.2`. This can be used to test flag
submission locally without setting up a network.

### API
- GET /teams
```
[
  {
    "team_id": "3",
    "name": "team-2",
    "ip": "10.42.0.3"
  },
  {
    "team_id": "1",
    "name": "team-0",
    "ip": "10.42.0.1"
  },
  {
    "team_id": "2",
    "name": "team-1",
    "ip": "10.42.0.2"
  }
]
```

- GET /services
```
[
  {
    "service_id": "1",
    "name": "challenge-1",
    "port": 1337
  },
  {
    "service_id": "2",
    "name": "challenge-2",
    "port": 1338
  }
]
```

- GET /service_state
```
[
 {
   "team_id": 2,
   "service_states": [
     {
       "service_id": 1,
       "state": "UP",
       "reason": null
     },
     {
       "service_id": 2,
       "state": "UP",
       "reason": null
     }
   ]
 }
]
```

- GET /history/flag/<team_id>
```
[
  {
    "service_id": "1",
    "flag_gained": [0, 1, 0, 0, 0],
    "flag_lost":   [0, 0, 0, 0, 0]
  }
]
```
- GET /history/service_state/<team_id>/<service_id>
```
// Unknown => 1,
// Up => 2,
// Down => 3,
// Mumble => 4,
// Corrupt => 5
// List [ ServiceState ]
[3,2,3,3,3]
```

- GET /status
```
{
  "name": "test",
  "flag_life": 30,
  "tick_time": 30,
  "current_round": 6,
  "flag_port": 8080,
  "total_round": 300,
  "round_started_at": "2023-08-07T20:12:35.981351886Z"
}
```

- GET /scoreboard
```
[
  {
    "team_id": 3,
    "position": 1,
    "total_point": 7.6602540378443855,
    "attack_point": 0.0,
    "defence_point": -1.0,
    "sla": 8.660254037844386,
    "service_score": [
      {
        "service_id": 1,
        "attack_point": 0.0,
        "defence_point": -1.0,
        "sla": 8.660254037844386
      }
    ]
  },
  {
    "team_id": 1,
    "position": 2,
    "total_point": 3.732050807568877,
    "attack_point": 2.0,
    "defence_point": 0.0,
    "sla": 1.7320508075688772,
    "service_score": [
      {
        "service_id": 1,
        "attack_point": 2.0,
        "defence_point": 0.0,
        "sla": 1.7320508075688772
      }
    ]
  },
  {
    "team_id": 2,
    "position": 3,
    "total_point": 1.7320508075688772,
    "attack_point": 0.0,
    "defence_point": 0.0,
    "sla": 1.7320508075688772,
    "service_score": [
      {
        "service_id": 1,
        "attack_point": 0.0,
        "defence_point": 0.0,
        "sla": 1.7320508075688772
      }
    ]
  }
]
```