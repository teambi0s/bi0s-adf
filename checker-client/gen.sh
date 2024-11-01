#!/usr/bin/env bash

set -x
# pip3 install grpcio grpcio-tools
python3 -m grpc_tools.protoc -I proto/ --python_out=. --grpc_python_out=. --mypy_out=.  checker.proto

