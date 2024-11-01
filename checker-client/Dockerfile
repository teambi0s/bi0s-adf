FROM python


RUN python -m pip install --upgrade pip
RUN pip install grpcio grpcio-tools

RUN mkdir /app
WORKDIR /app

COPY proto /app/proto/
COPY server.py /app/
RUN python -m grpc_tools.protoc -I /app/proto --python_out=. --grpc_python_out=. checker.proto
ENTRYPOINT ["python","/app/server.py"]

