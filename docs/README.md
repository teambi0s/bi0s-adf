# AD CTF

## How to Play? – Attack-Defense Capture The Flag (AD CTF)

### **Introduction**
The Attack-Defense Capture The Flag (AD CTF) is an advanced cybersecurity competition where teams are tasked with defending their own services while attacking the services of other teams. The goal is to secure your systems and exploit vulnerabilities in others. This guide explains the rules, setup, and strategies to help you successfully participate in the AD CTF.

For a high-level overview of how Attack-Defense CTFs work, you can watch this [LiveOverflow video](https://www.youtube.com/watch?v=RXgp4cDbiq4).

### **Overview**
In the AD CTF, each team is given access to a virtual machine (VM) running multiple services, each containing one or more vulnerabilities. Teams must protect their services from being exploited while attempting to compromise the services of other teams to capture their flags. The combined offensive and defensive performance will determine the final score of each team.

### **Accessing Your VM**
At the start of the game, each team will be provided with:
- An **SSH private key** and an **IP address** to access their VM.
- A **cloud-based VM** pre-configured with services that need to be protected and exploited.

To access your VM:
1. Use the provided SSH key and IP address to log into your VM.
2. After initial access, teams may add their own SSH keys for future logins.

### **Network Configuration**
Each VM contains an **OpenVPN configuration file** (client.conf), which is necessary to connect to the game network. Teams must:
1. Use this configuration file to establish a connection to the game network using a VPN.
2. Ensure all team members connect to the VPN to participate fully in the competition.

### **Game Services**
Each VM runs multiple **services** (e.g., web applications, command-line tools), which contain intentionally vulnerable code. Teams can:
- Analyze, modify, and patch these services to secure them.
- Exploit vulnerabilities in the services of other teams.

Each service may contain multiple vulnerabilities, and **patching** the service without disrupting its core functionality is essential. Uptime and availability of the services will impact your final score.

### **Flags**
A **flag** is a string of the format `^flag{\w{32}}$`. For example, a sample flag could look like:
```
flag{9f00da1ede821b88ff7cf779e8933ae9}
```

Teams earn points by:
- Capturing flags from other teams' services.
- Defending their own services and preventing their flags from being captured.

### **Game Ticks**
The competition is divided into multiple **ticks**, with each tick lasting three minutes. During each tick:
- New flags are deployed into the services by the game server.
- Flags are valid for 15 ticks before they expire, after which they can no longer be submitted.

### **Gameserver**
The **gameserver** is the central control system of the AD CTF. It manages the game by:
- Deploying flags into services.
- Conducting health checks on the services.
- Tracking the uptime (SLA) and score for each team.

### **Service Level Agreement (SLA)**
The **SLA** score measures the uptime of your services. It is calculated based on the proportion of time your services are in an operational state. A service can be in one of four states:
- **UP**: The service is functioning correctly, and flags can be retrieved.
- **MUMBLE**: The service is partially functional. SLA points are reduced.
- **CORRUPT**: The service is online, but flags cannot be retrieved. SLA points are further reduced.
- **DOWN**: The service is offline, and no points are awarded.

Maintaining high SLA scores requires ensuring that your services are consistently available and functioning correctly.

### **Flag Submission**
Flags are submitted by sending them to the **gameserver** over a TCP connection. The submission process is as follows:
1. Use the command `nc GAME_SERVER_IP 5555` to submit flags to the server.
2. Only flags from the past 15 ticks are valid; older flags will expire.
3. Submitting your own flags will not earn any points.

### **Game Rules**
To ensure fair play and a competitive environment, teams are required to follow these rules:

#### **Allowed Actions:**
- Patching your own services to fix vulnerabilities.
- Attacking other teams' services to capture flags.
- Modifying services without disabling their core functionality.

#### **Prohibited Actions:**
- Attacking the CTF infrastructure (e.g., gameserver, VPN, scoreboard).
- Launching denial-of-service (DoS) attacks or generating excessive traffic.
- Collaborating with other teams or sharing flags.
- Publishing solutions or flags during the competition (e.g., on GitHub, blogs).

Any violations of these rules may result in immediate disqualification.

### **Scoring System**
The total score is determined by the sum of three categories:
1. **Attack Points**: Earned by successfully exploiting other teams and capturing their flags.
2. **Defense Points**: Earned by successfully defending your own services from exploitation.
3. **SLA Points**: Earned by maintaining the uptime and functionality of your services.

### **Objectives**
To succeed in the AD CTF, teams must:
- Keep their services operational and secure.
- Identify and exploit vulnerabilities in other teams' services.
- Patch vulnerabilities in their own services to prevent flag capture.

### **API Endpoints**
The gameserver provides various API endpoints to facilitate monitoring and interaction. Sample API requests include:

- **GET /teams**:
  Retrieves a list of participating teams, their IDs, and IP addresses.

- **GET /services**:
  Lists the available services, their IDs, and the ports they run on.

- **GET /service_state**:
  Retrieves the state of each service for a specific team, indicating whether it is UP, MUMBLE, CORRUPT, or DOWN.

#### **Sample API Response:**

```json
[
  
]
```

_Note: API responses may vary slightly during the event._

### **Strategies for Success**
1. **Network Traffic Analysis**: Monitoring network traffic can help identify vulnerabilities in services that use insecure communication protocols.
2. **Automation**: Automating the process of flag capture and submission is key to maintaining a lead throughout the competition.
3. **Patching**: Applying patches efficiently is crucial, but be careful not to disrupt the core functionality of services. Always test patches thoroughly to avoid service downtime.

### **Conclusion**
The AD CTF is a challenging yet rewarding competition that tests both your defensive and offensive skills in a real-time environment. By understanding the rules, maximizing uptime, and strategically attacking other teams, you can secure a top position on the scoreboard.

Good luck, and may the best team win!
