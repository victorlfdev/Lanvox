"""Temporary TCP/UDP probe over the actual host path; requires Python and SSH.

Usage: python scripts/probe-media-network.py --ssh server@100.91.153.76
No packages, firewall changes, persistent server files or Lanvox credentials.
"""
import argparse
import json
import shlex
import socket
import struct
import subprocess
import threading
import time

SERVER = r'''
import json, socket, struct, threading, time
tcp = socket.socket(); tcp.bind(('0.0.0.0', 0)); tcp.listen(1); tcp.settimeout(15)
port = tcp.getsockname()[1]
udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM); udp.bind(('0.0.0.0', port))
udp.settimeout(0.1)
stats = [0, 0.0, None]; peer = [None]
def receive():
    while True:
        try: data, addr = udp.recvfrom(2048)
        except socket.timeout: continue
        except OSError: return
        peer[0] = addr
        if len(data) < 16: continue
        seq, sent = struct.unpack('!Qd', data[:16])
        transit = time.perf_counter() - sent
        stats[0] += 1
        if stats[2] is not None: stats[1] += (abs(transit-stats[2])-stats[1])/16
        stats[2] = transit
threading.Thread(target=receive, daemon=True).start()
print(json.dumps({'port': port}), flush=True)
conn, _ = tcp.accept(); conn.settimeout(20); stream = conn.makefile('rb')
block = bytes(65536)
try:
    for line in stream:
        cmd = json.loads(line)
        action = cmd['action']
        if action == 'download':
            left = cmd['bytes']
            while left:
                chunk = block[:min(left, len(block))]; conn.sendall(chunk); left -= len(chunk)
        elif action == 'upload':
            left = cmd['bytes']; start = time.perf_counter()
            while left:
                chunk = stream.read(min(left, len(block)))
                if not chunk: raise EOFError()
                left -= len(chunk)
            conn.sendall((json.dumps({'seconds': time.perf_counter()-start})+'\n').encode())
        elif action == 'reset':
            stats[:] = [0, 0.0, None]; conn.sendall(b'{}\n')
        elif action == 'stats':
            conn.sendall((json.dumps({'received': stats[0], 'jitter_ms': stats[1]*1000})+'\n').encode())
        elif action == 'reverse':
            count = cmd['count']; rate = cmd['mbps']*1e6/8; start = time.perf_counter()
            for seq in range(count):
                due = start + seq*1200/rate
                delay = due-time.perf_counter()
                if delay > 0: time.sleep(delay)
                udp.sendto(struct.pack('!Qd', seq, time.perf_counter())+bytes(1184), peer[0])
            conn.sendall((json.dumps({'sent': count, 'seconds': time.perf_counter()-start})+'\n').encode())
        elif action == 'stop': break
        else: conn.sendall(b'{}\n')
finally:
    conn.close(); tcp.close(); udp.close()
'''


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--ssh', required=True)
    parser.add_argument('--address')
    args = parser.parse_args()
    address = args.address or args.ssh.split('@')[-1]
    process = subprocess.Popen(
        ['ssh', '-o', 'BatchMode=yes', '-o', 'ConnectTimeout=10', args.ssh,
         'timeout 90 python3 -u -c ' + shlex.quote(SERVER)],
        stdout=subprocess.PIPE, text=True,
    )
    conn = udp = None
    try:
        ready = process.stdout.readline()
        if not ready:
            raise RuntimeError('Remote probe did not start')
        port = json.loads(ready)['port']
        conn = socket.create_connection((address, port), timeout=12)
        conn.settimeout(20)
        stream = conn.makefile('rb')

        def command(action, **kwargs):
            conn.sendall((json.dumps(dict(action=action, **kwargs))+'\n').encode())

        def response():
            return json.loads(stream.readline())

        latencies = []
        for _ in range(10):
            start = time.perf_counter(); command('ping'); response()
            latencies.append((time.perf_counter()-start)*1000)
        print(json.dumps({'tcp_rtt_ms_min': min(latencies),
                          'tcp_rtt_ms_mean': sum(latencies)/len(latencies)}), flush=True)
        size = 32*1024*1024
        start = time.perf_counter(); command('download', bytes=size)
        left = size
        while left:
            chunk = stream.read(min(left, 65536))
            if not chunk:
                raise EOFError('Download interrupted')
            left -= len(chunk)
        print(json.dumps({'host_to_pc_tcp_mbps': size*8/(time.perf_counter()-start)/1e6}), flush=True)
        command('upload', bytes=size); block = bytes(65536)
        for _ in range(size//len(block)):
            conn.sendall(block)
        print(json.dumps({'pc_to_host_tcp_mbps': size*8/response()['seconds']/1e6}), flush=True)

        udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        udp.settimeout(0.1)
        udp.connect((address, port))
        received = [0, 0.0, None]
        stop = threading.Event()

        def receive():
            while not stop.is_set():
                try:
                    data = udp.recv(2048)
                except socket.timeout:
                    continue
                _, sent = struct.unpack('!Qd', data[:16])
                transit = time.perf_counter()-sent
                received[0] += 1
                if received[2] is not None:
                    received[1] += (abs(transit-received[2])-received[1])/16
                received[2] = transit

        thread = threading.Thread(target=receive, daemon=True)
        thread.start()
        for mbps in [12, 24, 40]:
            count = int(mbps*1e6/8*2/1200)
            command('reset'); response()
            start = time.perf_counter()
            for seq in range(count):
                delay = start+seq*1200/(mbps*1e6/8)-time.perf_counter()
                if delay > 0:
                    time.sleep(delay)
                udp.send(struct.pack('!Qd', seq, time.perf_counter())+bytes(1184))
            seconds = time.perf_counter()-start
            time.sleep(0.3); command('stats'); result = response()
            print(json.dumps(dict(direction='pc_to_host_udp', target_mbps=mbps,
                                  sent_mbps=count*1200*8/seconds/1e6,
                                  loss_pct=100*(count-result['received'])/count,
                                  jitter_ms=result['jitter_ms'])), flush=True)
            received[:] = [0, 0.0, None]
            command('reverse', count=count, mbps=mbps); result = response()
            time.sleep(0.3)
            print(json.dumps(dict(direction='host_to_pc_udp', target_mbps=mbps,
                                  sent_mbps=count*1200*8/result['seconds']/1e6,
                                  loss_pct=100*(count-received[0])/count,
                                  jitter_ms=received[1]*1000)), flush=True)
        stop.set(); thread.join(timeout=1)
        command('stop')
    finally:
        if conn:
            conn.close()
        if udp:
            udp.close()
        try:
            process.wait(timeout=3)
        except subprocess.TimeoutExpired:
            process.kill()


if __name__ == '__main__':
    main()
