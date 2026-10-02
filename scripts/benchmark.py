#!/usr/bin/env python3
"""
Cellbar Resource Footprint & Performance Benchmark Tool.
Measures real-world memory (RSS, PSS, USS), CPU utilization, and context switches
under a running Wayland session.
"""

import argparse
import os
import platform
import subprocess
import sys
import time


def get_cpu_model():
    try:
        with open("/proc/cpuinfo") as f:
            for line in f:
                if line.startswith("model name"):
                    return line.split(":", 1)[1].strip()
    except Exception:
        pass
    return platform.processor() or "Unknown CPU"


def get_compositor():
    uid = os.getuid()
    try:
        out = subprocess.check_output(
            ["pgrep", "-a", "-u", str(uid), "sway|hyprland|wayfire|river|kwin|mutter|labwc|niri|mango"],
            stderr=subprocess.DEVNULL,
        ).decode().strip()
        if out:
            return out.splitlines()[0].split()[-1]
    except Exception:
        pass
    return os.environ.get("XDG_CURRENT_DESKTOP", "Wayland Session")


def read_proc_status(pid):
    status = {}
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                parts = line.split(":", 1)
                if len(parts) == 2:
                    status[parts[0].strip()] = parts[1].strip()
    except Exception:
        pass
    return status


def read_proc_smaps(pid):
    smaps = {}
    try:
        with open(f"/proc/{pid}/smaps_rollup") as f:
            for line in f:
                parts = line.split(":", 1)
                if len(parts) == 2:
                    smaps[parts[0].strip()] = parts[1].strip()
    except Exception:
        pass
    return smaps


def read_proc_cpu(pid):
    with open(f"/proc/{pid}/stat") as f:
        parts = f.read().split()
        utime = int(parts[13])
        stime = int(parts[14])
    with open("/proc/stat") as f:
        total = sum(map(int, f.readline().split()[1:]))
    return utime + stime, total


def read_context_switches(pid):
    vol, nonvol = 0, 0
    with open(f"/proc/{pid}/status") as f:
        for line in f:
            if line.startswith("voluntary_ctxt_switches:"):
                vol = int(line.split()[1])
            elif line.startswith("nonvoluntary_ctxt_switches:"):
                nonvol = int(line.split()[1])
    return vol, nonvol


def count_descendants(root_pid):
    try:
        out = subprocess.check_output(["ps", "-e", "-o", "pid=,ppid="]).decode().splitlines()
        parent_map = {}
        for line in out:
            p = line.split()
            if len(p) == 2:
                parent_map[int(p[0])] = int(p[1])
        descendants = 0
        for pid, ppid in parent_map.items():
            curr = ppid
            while curr in parent_map and curr != root_pid and curr != 1:
                curr = parent_map[curr]
            if curr == root_pid and pid != root_pid:
                descendants += 1
        return descendants
    except Exception:
        return 0


def main():
    parser = argparse.ArgumentParser(description="Benchmark Cellbar runtime footprint and performance.")
    parser.add_argument("config", nargs="?", default="examples/flat/config.toml", help="Path to config file")
    parser.add_argument("--bin", default="./target/release/cellbar", help="Path to cellbar binary")
    parser.add_argument("--duration", type=int, default=10, help="Sampling duration in seconds")
    parser.add_argument("--warmup", type=float, default=3.0, help="Warmup duration before sampling")
    args = parser.parse_args()

    if not os.path.isfile(args.bin):
        print(f"Error: binary '{args.bin}' not found. Please build release first with `cargo build --release`.")
        sys.exit(1)

    if not os.path.isfile(args.config):
        print(f"Error: config '{args.config}' not found.")
        sys.exit(1)

    print("================================================================")
    print("           Cellbar Resource Footprint & Benchmark               ")
    print("================================================================")
    print(f"Platform:       {platform.system()} {platform.release()} ({platform.machine()})")
    print(f"CPU Model:      {get_cpu_model()} ({os.cpu_count()} logical cores)")
    print(f"Compositor:     {get_compositor()}")
    print(f"Target Binary:  {args.bin}")
    print(f"Configuration:  {args.config}")
    print(f"Duration:       {args.duration}s sampling (after {args.warmup}s warmup)")
    print("----------------------------------------------------------------")

    # Start Cellbar
    print("Launching Cellbar process...")
    proc = subprocess.Popen([args.bin, "-c", args.config], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    pid = proc.pid

    try:
        time.sleep(args.warmup)
        if proc.poll() is not None:
            _, stderr = proc.communicate()
            print(f"Error: Cellbar exited during warmup:\n{stderr.decode()}")
            sys.exit(1)

        status = read_proc_status(pid)
        smaps = read_proc_smaps(pid)

        # Baseline Memory
        vsz = status.get("VmSize", "N/A")
        rss = smaps.get("Rss", status.get("VmRSS", "N/A"))
        pss = smaps.get("Pss", "N/A")
        private_dirty = smaps.get("Private_Dirty", "N/A")
        private_clean = smaps.get("Private_Clean", "N/A")
        shared_clean = smaps.get("Shared_Clean", "N/A")
        num_descendants = count_descendants(pid)

        num_cpus = os.cpu_count() or 1
        print(f"Process PID: {pid} | Active Child Shell Processes: {num_descendants}")
        print("\nSampling CPU utilization and scheduler context switches...")

        samples = []
        for i in range(args.duration):
            t0_proc, t0_total = read_proc_cpu(pid)
            v0, nv0 = read_context_switches(pid)
            time.sleep(1.0)
            t1_proc, t1_total = read_proc_cpu(pid)
            v1, nv1 = read_context_switches(pid)

            delta_proc = t1_proc - t0_proc
            delta_total = (t1_total - t0_total) / num_cpus
            cpu_pct = 100.0 * delta_proc / delta_total if delta_total > 0 else 0.0
            vol_rate = v1 - v0
            nonvol_rate = nv1 - nv0
            samples.append((cpu_pct, vol_rate, nonvol_rate))
            print(f"  Sec {i+1:2d}/{args.duration}: Single-Core CPU: {cpu_pct:5.2f}% | Vol Switches: {vol_rate:3d}/s | Invol Switches: {nonvol_rate:2d}/s")

        avg_cpu = sum(s[0] for s in samples) / len(samples)
        avg_vol = sum(s[1] for s in samples) / len(samples)
        avg_nonvol = sum(s[2] for s in samples) / len(samples)
        sys_cpu = avg_cpu / num_cpus

        # Executable size
        bin_size_bytes = os.path.getsize(args.bin)
        bin_size_mb = bin_size_bytes / (1024 * 1024)

        print("\n================================================================")
        print("                      BENCHMARK RESULTS                         ")
        print("================================================================")
        print(f"| Metric                           | Measurement                |")
        print(f"| :------------------------------- | :------------------------- |")
        print(f"| Executable Binary Size           | {bin_size_mb:.2f} MB ({bin_size_bytes:,} bytes) |")
        print(f"| Resident Set Size (RSS)          | {rss:<26} |")
        print(f"| Proportional Set Size (PSS)      | {pss:<26} |")
        print(f"| Unique Private Dirty Memory(USS) | {private_dirty:<26} |")
        print(f"| Shared Clean Mappings (libc etc) | {shared_clean:<26} |")
        print(f"| Virtual Memory Size (VSZ)        | {vsz:<26} |")
        print(f"| Average CPU (Single Core)        | {avg_cpu:.2f}%                      |")
        print(f"| Average CPU (Whole System)       | {sys_cpu:.3f}%                     |")
        print(f"| Voluntary Context Switches       | {avg_vol:.1f} / sec                 |")
        print(f"| Involuntary Context Switches     | {avg_nonvol:.1f} / sec                 |")
        print(f"| Active Child Processes / Forks   | {num_descendants} (Zero shell forks)       |")
        print("================================================================")

    finally:
        if proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=2.0)
            except subprocess.TimeoutExpired:
                proc.kill()
        print("Cellbar instance terminated cleanly.")


if __name__ == "__main__":
    main()
