#!/usr/bin/env python3

from concurrent import futures
from random import choices
import string
import sys

import grpc

import checker_pb2 as checker
import checker_pb2_grpc as checker_grpc


class Checker(checker_grpc.CheckerServicer):
    def __init__(self):
        self.ip_map = {}
        self.toggle = False

    def PlantFlag(self, request, context):
        print(
            "Planting Flag\nIp: {}:{}\nFlag : {}\nSlot : {}".format(
                request.ip, request.port, request.flag, request.slot
            )
        )

        # Write the flag to a file in ip:flag format, each entry on a new line
        with open("flags.txt", "a") as file:
            file.write(f"{request.ip}:{request.flag}\n")

        # time.sleep(random.randint(0,20))
        sys.stdout.flush()

        # Create Flag Response according to the service state
        state = checker.ServiceState.UP
        reason = "UP"
        status = checker.ServiceStatus(state=state, reason=reason)

        # Generate random token of length 20 bytes
        token = "".join(choices(string.ascii_letters + string.digits, k=20))
        identifier = "".join(choices(string.ascii_letters + string.digits, k=20))

        return checker.PlantFlagResponse(
            status=status, token=token, identifier=identifier
        )

    def CheckFlag(self, request, context):
        print(
            "CheckFlag\nIp : {}:{}\nFlag :{}\nToken : {}\nSlot :{}\nIdentifier: {}".format(
                request.ip,
                request.port,
                request.flag,
                request.token,
                request.slot,
                request.identifier,
            )
        )

        state = choices([checker.ServiceState.UP, checker.ServiceState.CORRUPT])[0]
        state = checker.ServiceState.UP
        reason = "UP" if state == checker.ServiceState.UP else "Corrupt"

        return checker.ServiceStatus(state=state, reason=reason)

    def CheckService(self, request, context):
        print("CheckService\nIp: {}:{}\n".format(request.ip, request.port))

        ip = request.ip

        if ip not in self.ip_map:
            # If the map is empty, start with 1
            if not self.ip_map:
                self.ip_map[ip] = 1
            else:
                # Retrieve the last inserted value and increment it
                last_value = max(self.ip_map.values())
                self.ip_map[ip] = last_value + 1

        value = self.ip_map[ip]

        if value == 1:
            if self.toggle:
                state = checker.ServiceState.DOWN
                self.toggle = False
            else:
                state = checker.ServiceState.UP
                self.toggle = True
            reason = ""
        elif value == 2:
            state = choices(
                [
                    # checker.ServiceState.UP,
                    checker.ServiceState.DOWN,
                    checker.ServiceState.CORRUPT,
                    checker.ServiceState.MUMBLE,
                ]
            )[0]
            reason = "Random"
        else:
            state = checker.ServiceState.UP
            reason = ""

        return checker.ServiceStatus(state=state, reason=reason)


def serve():
    addr = "[::]:50051"
    server = grpc.server(futures.ThreadPoolExecutor(max_workers=10))
    checker_grpc.add_CheckerServicer_to_server(Checker(), server)
    server.add_insecure_port(addr)
    server.start()
    print("Server Started On : {}".format(addr))
    server.wait_for_termination()


if __name__ == "__main__":
    serve()
