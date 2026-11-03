"""One-shot sandbox acceptance; never installs production units or touches PM2.

Default is read-only capability inspection. --execute-transient creates at most
two collected systemd units with DynamicUser and private /run directories.
"""
import argparse
import json
import pathlib
import socket
import subprocess
import uuid

POLICY_KEYS = (
    'CPUQuota', 'MemoryHigh', 'MemoryMax', 'TasksMax', 'Nice',
    'IOSchedulingClass', 'NoNewPrivileges', 'ProtectSystem', 'ProtectHome',
    'PrivateTmp', 'PrivateDevices', 'ProtectKernelTunables', 'ProtectKernelModules',
    'ProtectControlGroups', 'RestrictSUIDSGID', 'LockPersonality',
    'CapabilityBoundingSet', 'RestrictAddressFamilies', 'IPAddressDeny',
    'BindReadOnlyPaths',
)


def policy(unit_text):
    rows = []
    for line in unit_text.splitlines():
        key, sep, value = line.partition('=')
        if sep and key in POLICY_KEYS:
            rows.append((key, value))
    if set(POLICY_KEYS) != {k for k, _ in rows}:
        raise ValueError('incomplete_sandbox_policy')
    return rows


CHILD = r'''
import errno,json,os,pathlib,signal,socket,time,urllib.request
out={'uid_nonroot':os.getuid()!=0,'etc_readonly':bool(os.statvfs('/etc').f_flag & os.ST_RDONLY)}
status=dict(line.split(':',1) for line in pathlib.Path('/proc/self/status').read_text().splitlines() if ':' in line)
out['no_new_privileges']=status.get('NoNewPrivs','').strip()=='1'
out['effective_capabilities_zero']=int(status['CapEff'].strip(),16)==0
try: os.listdir('/home');out['home_hidden']=False
except PermissionError: out['home_hidden']=True
cg=next(line.split(':',2)[2] for line in pathlib.Path('/proc/self/cgroup').read_text().splitlines() if line.startswith('0::'))
base=pathlib.Path('/sys/fs/cgroup')/cg.lstrip('/')
values={name:(base/name).read_text().strip() for name in ['cpu.max','memory.max','memory.high','pids.max']}
quota,period=map(int,values['cpu.max'].split())
out['kernel_limits']={'cpu_quota_cores':quota/period,'memory_max_bytes':int(values['memory.max']),'memory_high_bytes':int(values['memory.high']),'tasks_max':int(values['pids.max'])}
out['limits_applied']=quota/period==0.2 and int(values['memory.max'])==192*1024*1024 and int(values['memory.high'])==128*1024*1024 and int(values['pids.max'])==24
sock=socket.socket();sock.settimeout(2)
try: sock.connect(('169.254.169.254',80));out['metadata_blocked_by_policy']=False
except OSError as e: out['metadata_blocked_by_policy']=e.errno in (errno.EPERM,errno.EACCES) or isinstance(e,socket.timeout);out['metadata_connect_errno']=e.errno;out['metadata_timeout']=isinstance(e,socket.timeout)
finally:sock.close()
def timeout(signum, frame):raise TimeoutError('dns_timeout')
signal.signal(signal.SIGALRM,timeout);signal.alarm(6)
try:out['official_dns_works']=bool(socket.getaddrinfo('www.twse.com.tw',443,type=socket.SOCK_STREAM))
except (OSError,TimeoutError):out['official_dns_works']=False
finally:signal.alarm(0)
https={}
for market,url in [('TWSE','https://www.twse.com.tw/exchangeReport/MI_MARGN?response=json&date=20261022&selectType=ALL'),('TPEX','https://www.tpex.org.tw/www/zh-tw/margin/balance?date=2026/10/22&id=&response=json')]:
    try:
        with urllib.request.urlopen(url,timeout=8) as response:https[market]=response.status if response.read(32).lstrip().startswith(b'{') else 'not_json'
    except Exception as e:https[market]=type(e).__name__
out['official_https_status']=https
out['official_https_works']=all(value==200 for value in https.values())
runtime=pathlib.Path(os.environ['RUNTIME_DIRECTORY']).resolve()
assert str(runtime).startswith('/run/tw-margin-radar-isolation-')
canary=runtime/'canary'
with canary.open('xb') as f:f.write(b'isolated-test-only')
out['private_runtime_writable']=canary.read_bytes()==b'isolated-test-only';canary.unlink()
out['passed']=all(out[k] for k in ['uid_nonroot','etc_readonly','no_new_privileges','effective_capabilities_zero','home_hidden','limits_applied','metadata_blocked_by_policy','official_dns_works','official_https_works','private_runtime_writable'])
print(json.dumps(out),flush=True)
raise SystemExit(0 if out['passed'] else 1)
'''


def run_transient(unit_text, resolv_text):
    rows = policy(unit_text)
    # Refuse resource tests when the host lacks headroom. No global memory stress.
    mem = dict((line.split(':')[0], int(line.split()[1])) for line in pathlib.Path('/proc/meminfo').read_text().splitlines())
    if mem['MemAvailable'] < 384 * 1024:
        raise RuntimeError('less_than_384MiB_available_skip_transient_tests')
    token = uuid.uuid4().hex[:12]
    runtime = 'tw-margin-radar-isolation-' + token
    # TCP only, no HTTP request/Metadata-Flavor header and no metadata retrieval.
    with socket.socket() as baseline:
        baseline.settimeout(2)
        try:baseline.connect(('169.254.169.254',80));metadata_reachable=True
        except OSError:metadata_reachable=False
    if not metadata_reachable:
        raise RuntimeError('metadata_TCP_baseline_unreachable_cannot_prove_policy_block')
    resolver = '/run/' + runtime + '.resolv.conf'
    create = "import os,sys;from pathlib import Path;p=Path(" + repr(resolver) + ");f=p.open('xb');f.write(sys.stdin.buffer.read());f.close();os.chmod(p,0o644)"
    subprocess.run(['sudo','-n','/usr/bin/python3','-B','-c',create],input=resolv_text,text=True,check=True,timeout=10)
    try:
        return execute_units(rows,runtime,resolver)
    finally:
        remove = "from pathlib import Path;Path(" + repr(resolver) + ").unlink(missing_ok=True)"
        subprocess.run(['sudo','-n','/usr/bin/python3','-B','-c',remove],check=True,timeout=10)


def execute_units(rows,runtime,resolver):
    base = ['sudo', '-n', 'systemd-run', '--wait', '--pipe', '--collect', '--service-type=exec',
            '--unit=' + runtime, '--property=DynamicUser=yes', '--property=RuntimeDirectory=' + runtime,
            '--property=RuntimeDirectoryMode=0700', '--property=TimeoutStartSec=30',
            '--property=RuntimeMaxSec=30', '--property=TimeoutStopSec=5',
            '--property=ReadWritePaths=/run/' + runtime]
    for key, value in rows:
        if key == 'BindReadOnlyPaths':
            if value != '/etc/tw-margin-radar/resolv.conf:/etc/resolv.conf':
                raise ValueError('unexpected_resolver_bind_path')
            value = resolver + ':/etc/resolv.conf'
        base.append('--property=' + key + '=' + value)
    result = subprocess.run(base + ['/usr/bin/python3', '-B', '-c', CHILD], capture_output=True, text=True, timeout=45)
    if result.returncode:
        return {'passed': False, 'probe_returncode': result.returncode, 'diagnostic': result.stderr[-2500:], 'output': result.stdout[-2500:]}
    evidence = json.loads(result.stdout)
    evidence['metadata_baseline_tcp_reachable'] = True
    # A separate 48 MiB cgroup must contain its own deliberate 96 MiB allocation.
    oom_name = runtime + '-oom'
    oom = [arg if arg.startswith('--property=BindReadOnlyPaths=') else arg.replace(runtime, oom_name) for arg in base]
    oom += ['--property=MemoryMax=48M', '--property=MemoryHigh=infinity',
            '/usr/bin/python3', '-B', '-c', "x=bytearray(96*1024*1024);print('unexpected-allocation-success')"]
    failure = subprocess.run(oom, capture_output=True, text=True, timeout=45)
    evidence['memory_limit_contained_oom'] = failure.returncode != 0 and 'oom-kill' in failure.stderr
    evidence['oom_probe_returncode'] = failure.returncode
    if not evidence['memory_limit_contained_oom']:
        evidence['oom_diagnostic'] = failure.stderr[-1800:]
        evidence['oom_output'] = failure.stdout[-500:]
    evidence['passed'] = evidence['passed'] and evidence['memory_limit_contained_oom']
    # Both --collect jobs exited. Confirm neither test unit remains active.
    states = []
    for name in [runtime, oom_name]:
        p = subprocess.run(['systemctl', 'is-active', name + '.service'], capture_output=True, text=True, timeout=5)
        states.append(p.stdout.strip())
    evidence['transient_units_inactive'] = all(s in {'inactive', 'unknown', 'failed'} for s in states)
    evidence['passed'] = evidence['passed'] and evidence['transient_units_inactive']
    evidence['production_units_installed'] = False
    evidence['schedules_enabled'] = False
    evidence['pm2_control_calls'] = 0
    return evidence


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--execute-transient', action='store_true')
    args = parser.parse_args()
    if args.execute_transient:
        # SSH stdin caller supplies UNIT_TEXT from the reviewed checkout, avoiding remote files.
        text = globals().get('UNIT_TEXT')
        if text is None:
            text = (pathlib.Path(__file__).parent / 'systemd/tw-margin-radar@.service').read_text()
        resolv = globals().get('RESOLV_TEXT')
        if resolv is None:
            resolv = (pathlib.Path(__file__).parent / 'network/resolv.conf').read_text()
        result = run_transient(text, resolv)
        print(json.dumps(result))
        raise SystemExit(0 if result['passed'] else 1)
    print(json.dumps({'cgroup2': pathlib.Path('/sys/fs/cgroup/cgroup.controllers').exists(), 'transient_tests_executed': False}))
