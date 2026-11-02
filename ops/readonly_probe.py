"""Run via SSH stdin. No writes, credentials, order APIs, or process control."""
import json
import os
import pathlib
import shutil
import subprocess
import time


def cpu():
    return list(map(int, pathlib.Path('/proc/stat').read_text().splitlines()[0].split()[1:9]))


before = cpu()
time.sleep(3)
after = cpu()
delta = [b - a for a, b in zip(before, after)]
mem = {line.split(':')[0]: int(line.split()[1]) for line in pathlib.Path('/proc/meminfo').read_text().splitlines()}
disk = shutil.disk_usage('/')
result = {
    'captured_at_utc': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
    'cpu_count': os.cpu_count(), 'load_1_5_15': os.getloadavg(),
    'cpu_sample_seconds': 3, 'cpu_busy_pct': round(100 * (1 - (delta[3] + delta[4]) / max(sum(delta), 1)), 2),
    'ram_total_mib': mem['MemTotal'] // 1024, 'ram_available_mib': mem['MemAvailable'] // 1024,
    'swap_total_mib': mem['SwapTotal'] // 1024, 'swap_free_mib': mem['SwapFree'] // 1024,
    'disk_total_gib': round(disk.total / 2**30, 2), 'disk_free_gib': round(disk.free / 2**30, 2),
    'uptime_seconds': float(pathlib.Path('/proc/uptime').read_text().split()[0]),
    'production_writes': 0, 'process_control_calls': 0,
}
# jlist is used only if the existing daemon and its RPC socket are already live.
home = pathlib.Path.home()
pidfile = home / '.pm2/pm2.pid'
if pidfile.exists() and pathlib.Path('/proc', pidfile.read_text().strip()).exists() and (home / '.pm2/rpc.sock').exists():
    daemon_node = pathlib.Path('/proc', pidfile.read_text().strip(), 'exe').resolve()
    env = {'HOME': str(home), 'PM2_HOME': str(home / '.pm2'), 'PATH': str(daemon_node.parent) + ':/usr/bin:/bin'}
    proc = subprocess.run([str(daemon_node), str(daemon_node.parent / 'pm2'), 'jlist'], env=env, capture_output=True, text=True, timeout=15)
    if proc.returncode == 0:
        result['pm2'] = [
            {'name': p.get('name'), 'pid': p.get('pid'), 'status': p.get('pm2_env', {}).get('status'),
             'restart_count': p.get('pm2_env', {}).get('restart_time'),
             'started_epoch_ms': p.get('pm2_env', {}).get('pm_uptime'),
             'rss_mib': round(p.get('monit', {}).get('memory', 0) / 2**20, 2),
             'cpu_pct': p.get('monit', {}).get('cpu')}
            for p in json.loads(proc.stdout)
        ]
    else:
        result['pm2'] = {'error': 'readonly_rpc_failed'}
else:
    result['pm2'] = {'error': 'daemon_not_live_no_rpc_attempted'}
proc = subprocess.run(['systemctl', 'list-units', '--type=service', '--state=running', '--no-legend', '--plain', '--no-pager'], capture_output=True, text=True, timeout=10)
result['running_service_units'] = [line.split()[0] for line in proc.stdout.splitlines() if line.strip()]
print(json.dumps(result))
