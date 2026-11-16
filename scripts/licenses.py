"""Generate attribution for the locked Windows Rust dependency graph."""
import json,subprocess,sys,urllib.request
from pathlib import Path
out=Path(sys.argv[1]if len(sys.argv)>1 else'THIRD-PARTY-LICENSES.txt')
meta=json.loads(subprocess.check_output(['cargo','metadata','--locked','--format-version','1','--filter-platform','x86_64-pc-windows-msvc']))
used={p['id']for p in meta['resolve']['nodes']};pieces=['StreamSlice Lite - locked Rust dependency and runtime notices\nApplication: MIT; see LICENSE. FFmpeg/Streamlink are not bundled.\n']
for p in sorted(meta['packages'],key=lambda p:(p['name'],p['version'])):
 if p['id']not in used or p['name']=='streamslice-lite':continue
 root=Path(p['manifest_path']).parent;files=set()
 for pattern in ['LICENSE*','LICENCE*','COPYING*','NOTICE*','license*','licence*']:files.update(f for f in root.glob(pattern)if f.is_file())
 if p.get('license_file'):files.add(root/p['license_file'])
 pieces.append('\n'+'='*72+'\n'+p['name']+' '+p['version']+'\nDeclared license: '+str(p.get('license'))+'\nUpstream: '+str(p.get('repository')or'https://crates.io/crates/'+p['name'])+'\n')
 if not files:raise RuntimeError('Review missing attribution text for '+p['name'])
 for f in sorted(files):pieces.append('\n--- '+f.name+' ---\n'+f.read_text(errors='replace')+'\n')
version=subprocess.check_output(['rustc','--version'],text=True).split()[1]
for name in ['LICENSE-MIT','LICENSE-APACHE']:
 url='https://raw.githubusercontent.com/rust-lang/rust/'+version+'/'+name
 with urllib.request.urlopen(url,timeout=45)as r:data=r.read(5*1024*1024+1)
 if len(data)>5*1024*1024:raise RuntimeError('Unexpected runtime-notice size')
 pieces.append('\n--- Rust '+version+' '+name+' ('+url+') ---\n'+data.decode('utf-8',errors='replace')+'\n')
sysroot=Path(subprocess.check_output(['rustc','--print','sysroot'],text=True).strip())
runtime=sysroot/'share'/'doc'/'rust'/'COPYRIGHT-library.html'
if not runtime.is_file():runtime=sysroot/'share'/'doc'/'rust'/'COPYRIGHT.html'
if not runtime.is_file():raise RuntimeError('Rust runtime attribution is missing from this toolchain')
pieces.append('\n--- Rust standard-library copyright notices ---\n'+runtime.read_text(errors='replace')+'\n')
out.write_text(''.join(pieces),encoding='utf-8');print('Dependency/runtime notices:',out,out.stat().st_size,'bytes')
