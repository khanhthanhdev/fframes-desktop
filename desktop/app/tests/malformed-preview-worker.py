"""Deliberately corrupt M2 replies; rejection must reap this stalled child."""
import copy
import json
import socket
import struct
import sys
import time

def arg(flag):
    return sys.argv[sys.argv.index(flag) + 1]

def receive():
    length = struct.unpack(">I", sys.stdin.buffer.read(4))[0]
    return json.loads(sys.stdin.buffer.read(length))

def encoded(value):
    data = json.dumps(value).encode()
    return struct.pack(">I", len(data)) + data

def send(value):
    sys.stdout.buffer.write(encoded(value))
    sys.stdout.buffer.flush()

identity = dict(project_id="project", open_session="session", source_revision=arg("--revision"), worker_generation=int(arg("--generation")))
with socket.create_connection(("127.0.0.1", int(arg("--frame-port")))) as bulk:
    request = receive()
    send(dict(type="Hello", contract_version=1, supported_versions=[1], identity=identity, request_id=request["request_id"],
              fframes_version="1.1.0", runtime_version="0.1.0", sdk_version="test", backend="cpu",
              capabilities=["preview_identity_v1", "scaled_frame_v1", "inspect_v1", "prepared_audio_v1"],
              capability_gaps=["shader_preview"], max_frame_bytes=64 * 1024 * 1024, max_control_bytes=1024 * 1024,
              max_preview_width=1280, max_preview_height=720, audio_sample_rates=[48000], audio_sample_format="f32le_stereo_interleaved"))
    request = receive()
    timeline = dict(type="Timeline", envelope=request["envelope"] if "envelope" in request else {k: request[k] for k in ["identity", "request_id", "contract_version"]},
                    fps=30, width=2, height=2, total_frames=90, duration_seconds=3.0, scenes=[], audio_tracks=[])
    send(timeline)
    request = receive()
    envelope = request["envelope"]
    header = json.loads(arg("--header"))
    header.update(request_id=envelope["request_id"], frame_index=request["frame_index"])
    record = dict(kind="frame_rgba8", identity=identity, request_id=envelope["request_id"], offset=0, payload_len=16)
    response = dict(type="ScaledFrame", envelope=copy.deepcopy(envelope), header=header, record=record,
                    frame_index=request["frame_index"], seek_serial=request["seek_serial"], scale=request["scale"], render_duration_micros=10)
    case = arg("--case")
    if case == "envelope":
        response["envelope"]["identity"]["open_session"] = "old-session"
    elif case == "destination":
        response = dict(timeline, envelope=envelope)
    elif case == "geometry":
        header["payload_len"] = 1024 * 1024 * 1024
    elif case == "seek":
        response["seek_serial"] -= 1
    send(response)
    if case in {"bulk", "truncated"}:
        if case == "bulk":
            record["kind"] = "audio_pcm_f32_le"
        bulk.sendall(encoded(record))
        if case == "truncated":
            bulk.sendall(bytes(8))
            bulk.shutdown(socket.SHUT_WR)
    time.sleep(60)
