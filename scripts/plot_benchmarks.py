#!/usr/bin/env python3
import sys

def parse_benchmark(file_path):
    results = []
    
    with open(file_path, 'r') as f:
        headers = []
        for line in f:
            line = line.strip()
            if line.startswith('| Size'):
                headers = [h.strip() for h in line.split('|')[1:-1]]
            elif line.startswith('|') and not line.startswith('|---'):
                parts = [p.strip() for p in line.split('|')[1:-1]]
                if not headers or len(parts) != len(headers):
                    continue
                
                try:
                    size = int(parts[headers.index('Size')])
                    
                    time_idx = -1
                    for i, h in enumerate(headers):
                        if 'Time' in h:
                            time_idx = i
                            break
                    if time_idx == -1: continue
                    
                    time_str = parts[time_idx]
                    if '/' in time_str:
                        avg_time = float(time_str.split('/')[1])
                    else:
                        avg_time = float(time_str)
                        
                    results.append((size, avg_time))
                except Exception:
                    pass
    
    return results

if __name__ == '__main__':
    if len(sys.argv) < 2:
        print("Usage: plot_benchmarks.py <benchmark_history.md>")
        sys.exit(1)
        
    results = parse_benchmark(sys.argv[1])
    for s, t in results:
        print(f"N={s:4d} | Time={t:10.2f}s")
