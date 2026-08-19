import json, os, urllib.request, time, subprocess

HERE = os.path.dirname(os.path.abspath(__file__))
VALIDATOR = os.path.join(HERE, "validate_img.py")

prompt = "a colorful wooden spinning top toy, dark background, product photography"


def submit(idx, seed):
    wf = {
        "1": {"class_type": "CheckpointLoaderSimple", "inputs": {"ckpt_name": "v1-5-pruned-emaonly.safetensors"}},
        "2": {"class_type": "CLIPTextEncode", "inputs": {"text": prompt, "clip": ["1", 1]}},
        "3": {"class_type": "CLIPTextEncode", "inputs": {"text": "blurry", "clip": ["1", 1]}},
        "4": {"class_type": "EmptyLatentImage", "inputs": {"width": 512, "height": 512, "batch_size": 1}},
        "5": {"class_type": "KSampler", "inputs": {"seed": seed, "steps": 15, "cfg": 7, "sampler_name": "euler",
                                                   "scheduler": "normal", "denoise": 1, "model": ["1", 0],
                                                   "positive": ["2", 0], "negative": ["3", 0], "latent_image": ["4", 0]}},
        "6": {"class_type": "VAEDecode", "inputs": {"samples": ["5", 0], "vae": ["1", 2]}},
        "7": {"class_type": "SaveImage", "inputs": {"filename_prefix": "seq_" + str(idx).zfill(2), "images": ["6", 0]}},
    }
    req = urllib.request.Request("http://127.0.0.1:8188/prompt", data=json.dumps({"prompt": wf}).encode())
    pid = json.loads(urllib.request.urlopen(req).read())["prompt_id"]
    for _ in range(150):
        time.sleep(2)
        h = json.loads(urllib.request.urlopen("http://127.0.0.1:8188/history/" + pid).read())
        if pid in h:
            return
    raise SystemExit("timeout on " + str(idx))


# same seed every time: any difference between runs is corruption, not sampling
for i in range(4):
    submit(i, 42)
    out = subprocess.run(["bash", "-c", "ls -t /opt/ComfyUI/output/seq_" + str(i).zfill(2) + "*.png | head -1"],
                         capture_output=True, text=True).stdout.strip()
    if not out:
        print("run", i, "-> no output image found")
        continue
    r = subprocess.run(["python3", VALIDATOR, out], capture_output=True, text=True)
    print("run", i, "->", r.stdout.strip().replace("\n", " | "))
