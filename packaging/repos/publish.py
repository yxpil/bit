#!/usr/bin/env python3
"""把 /tmp/repos 下的 apt + pacman 仓库发布到 GitHub 仓库并启用 Pages。
用法: python3 publish.py /tmp/repos
"""
import base64
import json
import subprocess
import sys
from pathlib import Path

MAP = [("apt", "yxpil/apt-repo"), ("pacman", "yxpil/pacman-repo")]
README = {"apt": "/tmp/apt-repo-readme.md", "pacman": "/tmp/pacman-repo-readme.md"}


def gh(*args, input_data=None):
    cmd = ["gh", "api", *args]
    r = subprocess.run(cmd, capture_output=True, text=True, input=input_data)
    if r.returncode != 0:
        print("STDERR:", r.stderr[:500])
    return r.stdout


def upload(repo, rel, local: Path):
    b64 = base64.b64encode(local.read_bytes()).decode()
    # 已存在则先取 sha 以更新
    existing = gh(f"repos/{repo}/contents/{rel}")
    payload = {"message": f"publish {rel}", "content": b64}
    if existing:
        payload["sha"] = json.loads(existing)["sha"]
    gh("-X", "PUT", f"repos/{repo}/contents/{rel}",
       input_data=json.dumps(payload))
    print(f"  {repo}/{rel}  ({local.stat().st_size} bytes)")


def enable_pages(repo):
    # 已启用会 409，忽略
    subprocess.run(["gh", "api", "-X", "POST", f"repos/{repo}/pages",
                    "-f", "build_type=legacy",
                    "-f", "source[branch]=main", "-f", "source[path]=/"],
                   capture_output=True, text=True)


def main():
    root = Path(sys.argv[1])
    for sub, repo in MAP:
        print(f"== {repo} ==")
        subprocess.run(["gh", "repo", "create", repo, "--public",
                        "--description", f"软件源 for BIT (https://github.com/yxpil/bit)"],
                       capture_output=True, text=True)
        for f in sorted((root / sub).rglob("*")):
            if f.is_file():
                upload(repo, f.relative_to(root / sub).as_posix(), f)
        upload(repo, "README.md", Path(README[sub]))
        enable_pages(repo)
    print("done")


if __name__ == "__main__":
    main()
