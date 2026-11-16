"""Synthetic integration checks. Never contacts real Twitch/Groq or reads real keys."""
import argparse,json,os,re,subprocess,tempfile,time,urllib.request,urllib.error,shutil
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--exe',required=True);p.add_argument('--install-tools',action='store_true');args=p.parse_args()
root=Path(tempfile.mkdtemp(prefix='ss-smoke-',dir=str(Path.home())));env=os.environ.copy();env.pop('GROQ_API_KEY',None);env['STREAMSLICE_HOME']=str(root/'data');proc=None;checks=[]
def wait(f,seconds=180):
 end=time.monotonic()+seconds
 while time.monotonic()<end:
  if proc.poll()is not None:raise AssertionError('Engine exited: '+(root/'engine.log').read_text())
  try:
   v=f()
   if v:return v
  except (OSError,ValueError):pass
  time.sleep(.25)
 raise TimeoutError('Timed out; evidence: '+str(root))
def check(name):checks.append(name);print('PASS',name,flush=True)
def api(path,data=None,extra=None):
 h={'x-streamslice-token':token,'Content-Type':'application/json'};h.update(extra or {});r=urllib.request.Request(base+'/api/'+path,headers=h,data=None if data is None else json.dumps(data).encode())
 try:
  with urllib.request.urlopen(r,timeout=120)as f:return f.status,json.load(f)
 except urllib.error.HTTPError as e:
  raw=e.read().decode()
  try:v=json.loads(raw)
  except ValueError:v=raw
  return e.code,v
def state():return api('state')[1]
def finished(id):
 def f():
  j=next(j for j in state()['jobs']if j['id']==id)
  if j['stage']=='failed':raise AssertionError(j)
  return j if j['stage']in['done','skipped','cancelled']else None
 return wait(f,300)
def tool(name):
 exe=name+('.exe'if os.name=='nt'else'');v=shutil.which(exe)
 if v:return v
 return str(next((root/'data'/'dependencies'/'ffmpeg').rglob(exe)))
log=(root/'engine.log').open('w')
try:
 proc=subprocess.Popen([str(Path(args.exe).resolve()),'--no-browser'],env=env,stdout=log,stderr=subprocess.STDOUT)
 base=wait(lambda:json.loads((root/'data'/'run.json').read_text())['origin'])
 html=wait(lambda:urllib.request.urlopen(base).read().decode());token=re.search(r'name="app-token" content="([a-f0-9]+)"',html).group(1)
 wait(lambda:not state()['doctor']['checking']);check('Rust engine and authenticated loopback dashboard')
 assert api('state',extra={'x-streamslice-token':'wrong'})[0]==401
 assert api('state',extra={'Origin':'https://untrusted.invalid'})[0]==403
 assert api('state',extra={'Host':'untrusted.invalid'})[0]==403;check('Token, Origin and Host protections')
 if args.install_tools:
  code,v=api('setup',{'confirm':True});assert code==200,v
  wait(lambda:not state()['setup']['running'],600);assert not state()['setup']['error'],state()['setup'];check('Native Windows verified portable-helper setup')
 d=state()['doctor'];assert d['ffmpeg']and d['ffprobe']and d['x264']and d['subtitles'],d
 code,v=api('key',{'key':'test-key-not-a-real-secret-00001'});assert code==200,v
 assert 'test-key-'not in json.dumps(state())
 assert api('jobs',{'mode':'ai','seconds':30,'preset':'quality','layout':'fit','consent':True})[0]==400
 assert api('key',{'clear':True})[0]==200;check('No key echo and no AI without current consent')
 assert api('selftest',{'confirm':False})[0]==400
 code,v=api('selftest',{'confirm':True});assert code==200,v
 j=finished(v['id']);assert j['stage']=='done'and j['srt'];check('720p video + audio + burned captions self-test')
 ffmpeg=tool('ffmpeg');ffprobe=tool('ffprobe');fixture=root/'source.mp4'
 subprocess.run([ffmpeg,'-hide_banner','-loglevel','error','-f','lavfi','-i','testsrc2=size=640x360:rate=15','-f','lavfi','-i','sine=frequency=440:sample_rate=16000','-t','32','-c:v','libx264','-preset','ultrafast','-g','30','-pix_fmt','yuv420p','-threads','1','-c:a','aac',str(fixture)],check=True)
 body={'source':str(fixture),'quality':'best','authorized':False};assert api('start',body)[0]==400;body['authorized']=True
 code,v=api('start',body);assert code==200,v
 wait(lambda:state()['buffer_seconds']>=6,60);check('Authorized live-style disk buffering')
 assert api('start',body)[0]==400;check('Concurrent capture prevention')
 for preset,layout,size in [('copy','original',(640,360)),('balanced','crop',(720,1280)),('quality','fit',(1080,1920))]:
  code,v=api('jobs',{'mode':'save','seconds':15,'preset':preset,'layout':layout});assert code==200,v
  j=finished(v['id']);assert j['stage']=='done';file=root/'data'/'exports'/(j['id']+'.mp4')
  data=json.loads(subprocess.check_output([ffprobe,'-v','error','-show_entries','stream=codec_type,codec_name,width,height,avg_frame_rate','-of','json',str(file)]));video=next(s for s in data['streams']if s['codec_type']=='video');assert(video['width'],video['height'])==size,(preset,data)
  a,b=map(float,video['avg_frame_rate'].split('/'));assert abs(a/b-15)<.1,(preset,data)
  assert any(s['codec_type']=='audio'for s in data['streams']);check(preset+' export, audio, dimensions and source FPS')
  with urllib.request.urlopen(urllib.request.Request(base+j['video'],headers={'Range':'bytes=0-127'}))as f:assert f.status==206 and len(f.read())==128
 check('Ranged MP4 preview serving')
 code,v=api('jobs',{'mode':'save','seconds':15,'preset':'quality','layout':'fit'});assert code==200,v
 wait(lambda:next(j for j in state()['jobs']if j['id']==v['id'])['stage']=='exporting',60)
 assert api('jobs/'+v['id']+'/cancel',{})[0]==200
 assert finished(v['id'])['stage']=='cancelled';check('Active render cancellation')
 assert api('stop',{})[0]==200;assert not state()['capture']['active'];check('Clean capture stop and retained exports')
 assert api('quit',{})[0]==200;assert proc.wait(timeout=30)==0;check('Graceful shutdown')
 report={'platform':os.name,'checks':checks,'count':len(checks),'real_twitch_tested':False,'real_groq_tested':False};(root/'report.json').write_text(json.dumps(report,indent=2));print(json.dumps(report),flush=True)
finally:
 if proc and proc.poll()is None:
  try:api('quit',{});proc.wait(timeout=25)
  except Exception:proc.terminate();proc.wait(timeout=15)
 log.close()
