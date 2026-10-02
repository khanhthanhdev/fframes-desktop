"""A deliberately stalled process for deadline/cleanup regression testing."""
import socket
import sys
import time

port = int(sys.argv[sys.argv.index("--frame-port") + 1])
with socket.create_connection(("127.0.0.1", port)):
    time.sleep(60)
