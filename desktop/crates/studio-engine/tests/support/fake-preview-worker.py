#!/usr/bin/env python3
"""Deterministic M2 preview worker used by agent-tool and candidate tests.

Installed as the isolated worker binary by an injected compiler. Behaviour comes from
`fframes-fake-worker.json` in the working directory (the materialized project root), so
every revision can describe its own timeline, diagnostics and audio:

  frames, fps, width, height, scenes [[start,end]], tracks [[start_s,end_s]],
  inspect [{frame,severity,key,message}], render_fail_frame, audio
  (silent|tone|clip|misplaced), pixel (base byte), backend (default: the one `--renderer` asks
  for, `cpu` for `auto`), hang_frame (stops answering at that frame), fail_start, delay_ms,
  export_hang, timing (offer set_timing_v1: scenes are keyed s0, s1 ... and a timing document
  trims, splits and reorders them), editor_objects [{component, object, bounds: [x, y, width, height]}] (every
  frame carries them as exact-bounds editor metadata in video pixels), props (offer set_props_v1:
  every editor object is editable with the fields `{"fields": [PropField], "values": {..}}` and
  a props document's `pixel` value of any object replaces the base byte of the frames)

With `--export-v1` it stands in for the MP4 renderer, which it cannot be: it reports the
first frame rendered, then holds until it is killed (`export_hang`) or fails.
"""
import hashlib
import json
import math
import os
import socket
import struct
import sys
import time


def arg(flag, default=None):
    if flag in sys.argv:
        return sys.argv[sys.argv.index(flag) + 1]
    return default


CONFIG = {}
if os.path.exists("fframes-fake-worker.json"):
    with open("fframes-fake-worker.json") as handle:
        CONFIG = json.load(handle)

if CONFIG.get("fail_start"):
    sys.stderr.write("fake worker refuses to start\n")
    sys.exit(3)

if "--export-v1" in sys.argv:
    total = int(CONFIG.get("frames", 90))
    sys.stderr.write(json.dumps({"event": "progress", "done": 1, "total": total}) + "\n")
    sys.stderr.flush()
    while CONFIG.get("export_hang"):
        time.sleep(0.05)
    sys.stderr.write("fake worker cannot encode MP4\n")
    sys.exit(1)

identity = dict(
    project_id=arg("--project-id"),
    open_session=arg("--open-session"),
    source_revision=arg("--revision"),
    worker_generation=int(arg("--generation")),
)
FRAMES = int(CONFIG.get("frames", 90))
FPS = int(CONFIG.get("fps", 30))
WIDTH = int(CONFIG.get("width", 8))
HEIGHT = int(CONFIG.get("height", 4))
SCENES = CONFIG.get("scenes", [[0, FRAMES // 2], [FRAMES // 2, FRAMES]])
TRACKS = CONFIG.get("tracks", [])
RATE = 48000
MAX_WIDTH = 1280
MAX_HEIGHT = 720
TIMING = bool(CONFIG.get("timing"))
timing_doc = None
timing_generation = 0
PROPS = CONFIG.get("props")
props_doc = None
props_generation = 0
cache = arg("--audio-cache")
os.makedirs(cache, exist_ok=True)


def layout():
    """The scenes of the timeline: code order, or as the timing document lays them out."""
    keys = ["s%d" % i for i in range(len(SCENES))]
    clips, orphans, seen, used = [], [], {}, set()
    for clip in (timing_doc or {}).get("clips", []):
        base = clip["key"].split("#")[0]
        if base not in keys:
            orphans.append(clip["key"])
            continue
        i = keys.index(base)
        clips.append((i, seen.get(i, 0), clip["trim_start_frames"], clip["trim_end_frames"]))
        seen[i] = seen.get(i, 0) + 1
        used.add(i)
    appended = [keys[i] for i in range(len(keys)) if i not in used]
    clips += [(i, 0, 0, 0) for i in range(len(keys)) if i not in used]
    placed, cursor = [], 0
    for i, occurrence, trim_start, trim_end in clips:
        length = SCENES[i][1] - SCENES[i][0] - trim_start - trim_end
        placed.append((i, occurrence, trim_start, trim_end, cursor, cursor + length))
        cursor += length
    return placed, cursor, orphans, appended


def clip_key(scene, occurrence):
    """A scene's key, with the code-order suffix the real worker gives repeated ones."""
    return "s%d" % scene if occurrence == 0 else "s%d#%d" % (scene, occurrence + 1)


def receive():
    head = sys.stdin.buffer.read(4)
    if len(head) < 4:
        sys.exit(0)
    length = struct.unpack(">I", head)[0]
    return json.loads(sys.stdin.buffer.read(length))


def encoded(value):
    data = json.dumps(value).encode()
    return struct.pack(">I", len(data)) + data


def send(value):
    sys.stdout.buffer.write(encoded(value))
    sys.stdout.buffer.flush()


bulk = socket.create_connection(("127.0.0.1", int(arg("--frame-port"))))
artifacts = {}


def envelope_of(request):
    if "envelope" in request:
        return request["envelope"]
    return {k: request[k] for k in ["identity", "request_id", "contract_version"]}


def scaled(n, scale):
    if scale == 1:
        return n
    # Rust `f64::round` (half away from zero), as the real client computes dimensions.
    return max(int(math.floor(n * scale / 2 + 0.5)) * 2, 2)


def effective_scale(requested):
    """The real worker's clamp (fframes-studio-runtime preview_worker.rs): never above the
    1280x720 preview cap, and the response carries the clamped scale."""
    return min(requested, min(MAX_WIDTH / WIDTH, MAX_HEIGHT / HEIGHT, 1.0))


def editor_metadata(index, seek_serial):
    objects = [
        dict(identity=dict(scene_instance_key="scene-0", component_key=o["component"],
                           object_key=o["object"], repeat_key="primary"),
             parent=None, style_tokens=[],
             bounds=dict(x=o["bounds"][0], y=o["bounds"][1], width=o["bounds"][2], height=o["bounds"][3]),
             paint_order=order, support="exact_bounds")
        for order, o in enumerate(CONFIG["editor_objects"])
    ]
    keys = sorted(json.dumps(o["identity"], sort_keys=True) for o in objects)
    return dict(frame_index=index, seek_serial=seek_serial, video_width=WIDTH, video_height=HEIGHT,
                editor_index_digest=hashlib.sha256("\n".join(keys).encode()).hexdigest(),
                frame_geometry_digest=hashlib.sha256(json.dumps(objects, sort_keys=True).encode()).hexdigest(),
                status="supported", reason=None, objects=objects)


def props_values(identity):
    """An object's values after the props document: the code's, then the document's."""
    values = dict(PROPS.get("values", {}))
    for entry in (props_doc or {}).get("objects", []):
        if entry["identity"] == identity:
            values.update(entry["values"])
    return values


def props_pixel():
    """The `pixel` value the document gives any object, if it gives one."""
    for entry in (props_doc or {}).get("objects", []):
        if "pixel" in entry["values"]:
            return int(entry["values"]["pixel"])
    return None


def make_pcm(path):
    samples = -(-FRAMES * RATE // FPS)
    mode = CONFIG.get("audio", "silent")
    digest = hashlib.sha256()
    with open(path, "wb") as out:
        done = 0
        while done < samples:
            count = min(4096, samples - done)
            chunk = bytearray()
            for i in range(done, done + count):
                t = i / RATE
                value = 0.0
                if mode in ("tone", "clip"):
                    for start, end in TRACKS:
                        if start <= t < end:
                            amp = 1.5 if mode == "clip" else 0.4
                            value = max(-1.0, min(1.0, amp * math.sin(t * 440 * 2 * math.pi))) if mode == "clip" else amp * math.sin(t * 440 * 2 * math.pi)
                            if mode == "clip":
                                value = 1.0 if value > 0.99 else value
                elif mode == "misplaced" and FRAMES / FPS > 2.0 and 1.5 <= t < 2.0:
                    value = 0.4 * math.sin(t * 440 * 2 * math.pi)
                chunk += struct.pack("<ff", value, value)
            out.write(chunk)
            digest.update(chunk)
            done += count
    return samples, digest.hexdigest()


while True:
    request = receive()
    kind = request["type"]
    if CONFIG.get("request_log"):
        with open(CONFIG["request_log"], "a") as handle:
            handle.write(json.dumps(request) + "\n")
    if CONFIG.get("delay_ms"):
        time.sleep(CONFIG["delay_ms"] / 1000)
    if kind == "Hello":
        send(dict(
            type="Hello", contract_version=1, supported_versions=[1], identity=identity,
            request_id=request["request_id"], fframes_version="1.1.0", runtime_version="0.1.0",
            sdk_version=arg("--sdk-version", "test"),
            backend=CONFIG.get("backend") or {"skia-cpu": "skia-cpu", "gpu": "skia-gpu"}.get(arg("--renderer"), "cpu"),
            capabilities=["preview_identity_v1", "scaled_frame_v1", "inspect_v1", "prepared_audio_v1"]
            + (["set_timing_v1"] if TIMING else []) + (["set_props_v1"] if PROPS is not None else [])
            + (["suggested_stops_v1"] if "suggestions" in CONFIG else []),
            capability_gaps=CONFIG.get("capability_gaps", []), max_frame_bytes=64 * 1024 * 1024, max_control_bytes=1024 * 1024,
            max_preview_width=MAX_WIDTH, max_preview_height=MAX_HEIGHT, audio_sample_rates=[48000],
            audio_sample_format="f32le_stereo_interleaved"))
    elif kind == "SetTiming":
        env = request["envelope"]
        try:
            doc = json.loads(request["timing_json"]) if request["timing_json"] else None
        except ValueError:
            send(dict(type="Error", envelope=env, code="timing", message="fake timing document is not JSON"))
            continue
        timing_doc, timing_generation = doc, request["timing_generation"]
        _, total, orphans, appended = layout()
        send(dict(type="TimingApplied", envelope=env, timing_generation=timing_generation,
                  total_frames=total, orphan_clips=orphans, appended_scenes=appended))
    elif kind == "SetProps" and PROPS is not None:
        env = request["envelope"]
        try:
            doc = json.loads(request["props_json"]) if request["props_json"] else None
        except ValueError:
            send(dict(type="Error", envelope=env, code="props", message="fake props document is not JSON"))
            continue
        props_doc, props_generation = doc, request["props_generation"]
        send(dict(type="PropsApplied", envelope=env, props_generation=props_generation))
    elif kind == "PropsInfo" and PROPS is not None:
        env = request["envelope"]
        wanted = request["identity"]
        known = any(o["object"] == wanted["object_key"] for o in CONFIG.get("editor_objects", []))
        info = None
        if known:
            info = dict(schema=dict(component=wanted["component_key"], fields=PROPS.get("fields", [])),
                        code_values=PROPS.get("values", {}), values=props_values(wanted))
        send(dict(type="PropsInfo", envelope=env, identity=wanted, props_generation=props_generation,
                  info=info))
    elif kind == "Timeline" and TIMING:
        env = envelope_of(request)
        placed, total, _, _ = layout()
        send(dict(
            type="Timeline", envelope=env, fps=FPS, width=WIDTH, height=HEIGHT, total_frames=total,
            duration_seconds=total / FPS, timing_generation=timing_generation,
            scenes=[dict(instance_id="scene-%d-%d" % (i, n), editor_instance_key=clip_key(i, n), index=k,
                         name="s%d" % i, full_name="s%d" % i, start_frame=s, end_frame=e,
                         start_seconds=s / FPS, end_seconds=e / FPS,
                         clip=dict(timing_key=clip_key(i, n), clip_index=n, trim_start_frames=ts,
                                   trim_end_frames=te, scene_index=i,
                                   scene_frames=SCENES[i][1] - SCENES[i][0]))
                    for k, (i, n, ts, te, s, e) in enumerate(placed)],
            audio_tracks=[dict(file="cue.wav", start_seconds=s, end_seconds=e,
                               mix=dict(gain_db=0.0, pan=0.0, fade_in=0.0, fade_out=0.0, offset=0.0,
                                        voice=False, duck_under_voice=False))
                          for s, e in TRACKS]))
    elif kind == "Timeline":
        env = envelope_of(request)
        send(dict(
            type="Timeline", envelope=env, fps=FPS, width=WIDTH, height=HEIGHT, total_frames=FRAMES,
            duration_seconds=FRAMES / FPS,
            scenes=[dict(instance_id="scene-%d" % i, index=i, name="s%d" % i, full_name="s%d" % i,
                         start_frame=s, end_frame=e, start_seconds=s / FPS, end_seconds=e / FPS)
                    for i, (s, e) in enumerate(SCENES)],
            audio_tracks=[dict(file="cue.wav", start_seconds=s, end_seconds=e,
                               mix=dict(gain_db=0.0, pan=0.0, fade_in=0.0, fade_out=0.0, offset=0.0,
                                        voice=False, duck_under_voice=False))
                          for s, e in TRACKS]))
    elif kind == "ScaledFrame":
        env = request["envelope"]
        index = request["frame_index"]
        while CONFIG.get("hang_frame") == index:
            time.sleep(0.05)
        if CONFIG.get("render_fail_frame") == index:
            send(dict(type="Error", envelope=env, code="render", message="fake render failure"))
            continue
        scale = effective_scale(request["scale"])
        w, h = scaled(WIDTH, scale), scaled(HEIGHT, scale)
        pixel = props_pixel() if PROPS is not None else None
        base = (pixel if pixel is not None else int(CONFIG.get("pixel", 7))) + (index if CONFIG.get("vary_pixels") else 0)
        payload = bytes([base % 256, (base * 3) % 256, 11, 255]) * (w * h)
        header = dict(protocol_version=1, source_revision=identity["source_revision"],
                      worker_generation=identity["worker_generation"], request_id=env["request_id"],
                      frame_index=index, width=w, height=h, stride_bytes=w * 4, channel_order="Rgba8",
                      alpha_mode="Straight", color_space="Srgb", payload_len=len(payload))
        record = dict(kind="frame_rgba8", identity=identity, request_id=env["request_id"], offset=0,
                      payload_len=len(payload))
        response = dict(type="ScaledFrame", envelope=env, frame_index=index, seek_serial=request["seek_serial"],
                        scale=scale, header=header, render_duration_micros=5, record=record)
        if CONFIG.get("editor_objects"):
            response["editor_metadata"] = editor_metadata(index, request["seek_serial"])
        send(response)
        bulk.sendall(encoded(record) + payload)
    elif kind == "Inspect":
        env = request["envelope"]
        found = [d for d in CONFIG.get("inspect", []) if d["frame"] in request["frames"]]
        send(dict(type="Inspect", envelope=env, diagnostics=found, truncated=bool(CONFIG.get("truncated"))))
    elif kind == "SuggestStops":
        time.sleep(CONFIG.get("suggest_delay_ms", 0) / 1000)
        send(dict(type="SuggestedStops", envelope=request["envelope"],
                  timing_generation=request["timing_generation"],
                  suggestions=dict(stops=CONFIG.get("suggestions", []), truncated=False)))
    elif kind == "PrepareAudio":
        env = request["envelope"]
        nonce = "%x" % (int(time.time() * 1e6) & 0xFFFFFFFFFFFF)
        artifact_id = "pcm-" + nonce
        path = os.path.join(cache, artifact_id + ".pcm")
        samples, digest = make_pcm(path)
        artifacts[artifact_id] = path
        send(dict(type="PreparedAudio", envelope=env, artifact_id=artifact_id, sample_rate=RATE, channels=2,
                  sample_count=samples, byte_count=samples * 8, sha256=digest, silent=not TRACKS))
    elif kind == "ReadAudio":
        env = request["envelope"]
        with open(artifacts[request["artifact_id"]], "rb") as handle:
            handle.seek(request["offset"])
            payload = handle.read(request["length"])
        record = dict(kind="audio_pcm_f32_le", identity=identity, request_id=env["request_id"],
                      offset=request["offset"], payload_len=len(payload))
        send(dict(type="AudioRead", envelope=env, artifact_id=request["artifact_id"],
                  offset=request["offset"], record=record))
        bulk.sendall(encoded(record) + payload)
    elif kind in ("ReleaseAudio", "CancelAudio"):
        env = request["envelope"]
        path = artifacts.pop(request["artifact_id"], None)
        if path and os.path.exists(path):
            os.remove(path)
        send(dict(type="Ack", envelope=env, released=path is not None))
    elif kind == "Shutdown":
        send(dict(type="Ack", envelope=envelope_of(request), released=True))
        for path in artifacts.values():
            if os.path.exists(path):
                os.remove(path)
        break
