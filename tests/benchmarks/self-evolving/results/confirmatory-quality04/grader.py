#!/usr/bin/env python3
"""Private post-freeze external grader. Never copy into an agent workspace."""
import argparse
from collections import Counter
import csv
import importlib.util
import io
import json
import os
from pathlib import Path
import selectors
import signal
import sqlite3
import subprocess
import sys
import threading
import time
import unittest

# Only these two already frozen support files are permitted by the evaluator.
helper = Path(__file__).resolve().parents[2] / 'tests/benchmarks/self-evolving/fixtures/grade.py'
spec = importlib.util.spec_from_file_location('trusted_pilot_grade', helper)
g = importlib.util.module_from_spec(spec)
spec.loader.exec_module(g)
SEVERITIES = ('info','warning','error')
DATA_SEED = 20260905

def event(service='api', severity='info', message='hello', **extra):
    return dict(service=service,severity=severity,message=message,**extra)

def oracle(events, top=3):
    sev=Counter(e['severity'] for e in events); svc=Counter(e['service'] for e in events); msg=Counter(e['message'] for e in events)
    return {'total':len(events),'by_severity':{s:sev[s] for s in SEVERITIES},'by_service':dict(sorted(svc.items())),
            'top_messages':[{'message':m,'count':n} for m,n in sorted(msg.items(),key=lambda x:(-x[1],x[0]))[:top]]}

class R1(g.ReportCase):
    def test_aggregate_order_repeat_unicode_top_and_determinism(self):
        groups=[[event('api',message='雪'),event('web','error','x')],[event('web','warning','雪'),event('other',message='b')],[event('api',message='x')]]
        self.write(groups[0]); paths=[self.source]
        for i,events in enumerate(groups[1:]):
            p=self.directory/f'part-{i}.jsonl';p.write_text('\n'+ '\n\n'.join(json.dumps(e,ensure_ascii=False) for e in events)+'\n');paths.append(p)
        self.assertEqual(self.report(),oracle(groups[0]))
        for top in (0,3,20):
            args=['--input',paths[1],'--input',paths[2],'--top',top]
            self.assertEqual(self.report(*args),oracle(sum(groups,[]),top))
            before=self.output.read_bytes();self.report(*args);self.assertEqual(self.output.read_bytes(),before)
        self.assertEqual(self.report('--input',self.source,'--top',20),oracle(groups[0]*2,20))
    def test_late_errors_preserve_output_and_identify_file_line(self):
        self.write([event()]);later=self.directory/'later.jsonl';later.write_text('\n'+json.dumps(event(message='雪'))+'\n{bad\n')
        self.output.write_bytes(b'prior-not-json\n');r=self.report('--input',later,code=2)
        self.assertIn(later.name,r.stderr);self.assertIn('line 3',r.stderr);self.assertEqual(self.output.read_bytes(),b'prior-not-json\n')
        r=self.report('--input',self.directory/'missing.jsonl',code=2);self.assertIn('missing.jsonl',r.stderr);self.assertEqual(self.output.read_bytes(),b'prior-not-json\n')

class R2(g.ReportCase):
    def test_filter_union_intersection_empty_and_literal_names(self):
        events=[event(s,sev,m) for s,sev,m in [('api','info','x'),('api','warning','雪'),('API','error','x'),('雪','error','雪'),('web','warning','y'),('web','info','z')]]
        self.write(events);self.assertEqual(self.report(),oracle(events))
        for services in [('api','雪'),('api','api'),('missing',),('API',)]:
            for threshold in SEVERITIES:
                args=[v for s in services for v in ('--service',s)]+['--min-severity',threshold,'--top',20]
                selected=[e for e in events if e['service'] in services and SEVERITIES.index(e['severity'])>=SEVERITIES.index(threshold)]
                self.assertEqual(self.report(*args),oracle(selected,20))
        self.assertEqual(self.report('--min-severity','warning','--top',0),oracle([e for e in events if e['severity']!='info'],0))
    def test_excluded_invalid_and_unknown_threshold_preserve_output(self):
        self.write([event('chosen'),event('excluded','unknown')]);self.output.write_bytes(b'prior\n')
        self.report('--service','chosen',code=2);self.assertEqual(self.output.read_bytes(),b'prior\n')
        self.write([event()]);self.report('--min-severity','fatal',code=2);self.assertEqual(self.output.read_bytes(),b'prior\n')

class R3(g.ReportCase):
    def csv_report(self,*args,code=0):
        r=self.cli('incident_report',self.source,'--output',self.output,'--format','csv',*args,code=code)
        self.assertEqual(r.stdout,'')
        return list(csv.reader(io.StringIO(self.output.read_text()))) if code==0 else r
    def test_csv_quoting_counts_empty_top_and_json_compatibility(self):
        names=['normal','comma,name','quote"name','line\nname','雪']
        events=[event(name,severity) for i,name in enumerate(names) for severity in ['info']*i+['warning']*(i%2)+['error']*(5-i)]
        self.write(events);before=self.report();json_bytes=self.output.read_bytes();self.assertEqual(before,oracle(events))
        expected=[['service','info','warning','error','total']]
        for name in sorted(names):
            counts=[sum(e['service']==name and e['severity']==severity for e in events) for severity in SEVERITIES]
            expected.append([name,*map(str,counts),str(sum(counts))])
        self.assertEqual(self.csv_report(),expected);csv_bytes=self.output.read_bytes();self.assertEqual(self.csv_report('--top',0),expected);self.assertEqual(self.output.read_bytes(),csv_bytes)
        self.report();self.assertEqual(self.output.read_bytes(),json_bytes)
        self.write([]);self.assertEqual(self.csv_report(),[expected[0]])
    def test_late_invalid_and_unknown_format_preserve(self):
        self.source.write_text(json.dumps(event())+'\n{bad\n');self.output.write_bytes(b'old,csv\n');self.csv_report(code=2);self.assertEqual(self.output.read_bytes(),b'old,csv\n')
        self.write([]);self.report('--format','xml',code=2);self.assertEqual(self.output.read_bytes(),b'old,csv\n')

class R4(g.ReportCase):
    def stream(self,*args,code=0):
        r=g.bounded_run([sys.executable,'-m','incident_report',str(self.source),'--stream',*map(str,args)],cwd=g.WORKSPACE,env=self.env,timeout=15)
        self.assertEqual(r.returncode,code,r.stderr);self.assertNotIn('Traceback',r.stderr)
        if code==0:self.assertEqual(r.stderr,'')
        else:self.assertTrue(r.stderr.strip())
        return r
    def test_projection_order_prefix_failure_and_output_conflict(self):
        entries=[event(message='雪',extra=9),event('web','error','later')];self.write(entries)
        before=set(self.directory.iterdir());r=self.stream();self.assertEqual([json.loads(x) for x in r.stdout.splitlines()],[{k:e[k] for k in ('service','severity','message')} for e in entries]);self.assertEqual(set(self.directory.iterdir()),before)
        self.source.write_text(json.dumps(entries[0])+'\n\n{bad\n');r=self.stream(code=2);self.assertEqual([json.loads(x) for x in r.stdout.splitlines()],[event(message='雪')]);self.assertIn('line 3',r.stderr)
        self.output.write_bytes(b'old\n');r=self.stream('--output',self.output,code=2);self.assertEqual(r.stdout,'');self.assertEqual(self.output.read_bytes(),b'old\n')
        self.report(code=2);self.assertEqual(self.output.read_bytes(),b'old\n')
    def test_fifo_emits_before_eof(self):
        fifo=self.directory/'events.fifo';os.mkfifo(fifo)
        # Opening RDWR keeps FIFO alive without a writer-open race; close only after observing output.
        fd=os.open(fifo,os.O_RDWR|os.O_NONBLOCK)
        proc=subprocess.Popen([sys.executable,'-m','incident_report',str(fifo),'--stream'],cwd=g.WORKSPACE,env=self.env,stdout=subprocess.PIPE,stderr=subprocess.PIPE,start_new_session=True)
        try:
            os.write(fd,(json.dumps(event(message='incremental'))+'\n').encode())
            sel=selectors.DefaultSelector()
            for name,pipe in [('stdout',proc.stdout),('stderr',proc.stderr)]:
                os.set_blocking(pipe.fileno(),False);sel.register(pipe,selectors.EVENT_READ,name)
            collected={'stdout':bytearray(),'stderr':bytearray()};used=0;deadline=time.monotonic()+5;emitted=False
            while sel.get_map() or proc.poll() is None:
                self.assertLess(time.monotonic(),deadline,'streaming process exceeded generous 5-second phase deadline')
                for key,_ in sel.select(.1):
                    data=os.read(key.fileobj.fileno(),65536)
                    if not data:sel.unregister(key.fileobj);continue
                    used+=len(data);self.assertLessEqual(used,1024*1024,'stream output exceeded quota')
                    collected[key.data].extend(data)
                if not emitted and b'\n' in collected['stdout']:
                    self.assertEqual(json.loads(bytes(collected['stdout']).split(b'\n')[0]),event(message='incremental'))
                    emitted=True;os.close(fd);fd=None;deadline=time.monotonic()+5
                if proc.poll() is not None and not sel.get_map():break
            self.assertTrue(emitted,'no complete event line emitted before EOF')
            self.assertEqual((proc.returncode,bytes(collected['stderr'])),(0,b''))
            self.assertEqual([json.loads(line) for line in bytes(collected['stdout']).splitlines()],[event(message='incremental')])
        finally:
            if fd is not None:os.close(fd)
            g._helper._terminate(proc,include_nested=True)
            if 'sel' in locals():sel.close()
            proc.stdout.close();proc.stderr.close()

class QueueHelpers(g.QueueCase):
    def populated(self):
        self.queue('enqueue','done','{"done":true}','--now',0);v=self.queue('claim','w','--lease-seconds',10,'--now',0);self.queue('ack','done','w',v['lease_token'])
        self.queue('enqueue','dead','[]','--max-attempts',1,'--now',0);v=self.queue('claim','w','--lease-seconds',10,'--now',0);self.queue('fail','dead','w',v['lease_token'],'--error','permanent','--now',0)
        self.queue('enqueue','expired','{"雪":1}','--now',0);self.queue('claim','old','--lease-seconds',1,'--now',0)
        # Directly add active/queued through ordinary enqueue without expiring old leases.
        self.queue('enqueue','queued','[1,2]','--priority',-2,'--now',0)
        self.queue('enqueue','active','null','--priority',100,'--now',0);self.queue('claim','owner','--lease-seconds',100000000000,'--now',0)
    def race(self,commands):
        # Two separately bounded CLI processes wait until both have reached a filesystem barrier.
        gate=self.directory/'race-start'
        ready=[self.directory/f'race-ready-{i}' for i in range(len(commands))]
        bootstrap="import os,sys,time; from pathlib import Path; ready,gate=map(Path,sys.argv[1:3]); ready.touch(); deadline=time.monotonic()+10\nwhile not gate.exists():\n if time.monotonic()>deadline:raise SystemExit(9)\n time.sleep(.01)\nos.execv(sys.executable,[sys.executable,'-m','durable_queue',*sys.argv[3:]])"
        results=[None]*len(commands)
        def worker(i,args):
            results[i]=g.bounded_run([sys.executable,'-c',bootstrap,str(ready[i]),str(gate),*map(str,args)],cwd=g.WORKSPACE,env=self.env,timeout=25,max_output_bytes=1024*1024)
        threads=[threading.Thread(target=worker,args=(i,args)) for i,args in enumerate(commands)]
        for thread in threads:thread.start()
        deadline=time.monotonic()+5
        while not all(path.exists() for path in ready) and time.monotonic()<deadline:time.sleep(.01)
        synchronized=all(path.exists() for path in ready);gate.touch()
        for thread in threads:thread.join(30)
        self.assertTrue(synchronized,'both independent processes must reach start barrier')
        self.assertTrue(all(not thread.is_alive() for thread in threads))
        parsed=[]
        for result in results:
            self.assertIsNotNone(result);self.assertNotIn('Traceback',result.stderr);self.assertIn(result.returncode,(0,2),result.stderr)
            if result.returncode==0:
                self.assertEqual(result.stderr,'');self.assertEqual(len(result.stdout.splitlines()),1)
            else:self.assertEqual(result.stdout,'');self.assertTrue(result.stderr.strip())
            parsed.append((result.returncode,json.loads(result.stdout) if result.stdout else None,result.stderr))
        return parsed

class Q1(QueueHelpers):
    def test_cancel_queued_idempotent_and_preserve_others(self):
        self.populated();before=self.rows();self.assertEqual(self.queue('cancel','queued'),{'cancelled':True});self.assertEqual(self.rows(),{k:v for k,v in before.items() if k!='queued'})
        before=self.snapshot()
        self.assertEqual(self.queue('cancel','queued'),{'cancelled':False});self.assertEqual(self.snapshot(),before)
        self.assertEqual(self.queue('cancel','missing'),{'cancelled':False});self.assertEqual(self.snapshot(),before)
        for id in ('done','dead','expired','active'):
            self.queue('cancel',id,code=2);self.assertEqual(self.snapshot(),before)
    def test_cancel_cannot_also_remove_successfully_claimed_row(self):
        self.queue('enqueue','job','{}','--now',0)
        cancel,claim=self.race([['cancel',self.database,'job'],['claim',self.database,'worker','--lease-seconds',10,'--now',0]])
        self.assertEqual(claim[0],0)
        if cancel[0]==0:
            self.assertEqual(cancel[1],{'cancelled':True});self.assertIsNone(claim[1]);self.assertEqual(self.rows(),{})
        else:
            self.assertEqual(cancel[0],2);self.assertEqual(claim[1]['task_id'],'job');self.assertEqual(self.rows()['job']['status'],'leased')

class Q2(QueueHelpers):
    def lease(self):
        self.queue('enqueue','job','{"payload":[1,"雪"]}','--now',0)
        return self.queue('claim','worker','--lease-seconds',10,'--now',1)
    def test_extension_nonshortening_fractional_and_fencing(self):
        v=self.lease();before=self.rows()['job'];token=v['lease_token']
        for seconds,now,expiry in [(20.5,2.25,22.75),(1,3,22.75)]:
            got=self.queue('extend','job','worker',token,'--lease-seconds',seconds,'--now',now)
            self.assertEqual(got,{'task_id':'job','lease_token':token,'lease_expires_at':expiry})
            self.assertEqual({k:x for k,x in self.rows()['job'].items() if k!='lease_expires_at'},{k:x for k,x in before.items() if k!='lease_expires_at'})
        self.assertIsNone(self.queue('claim','other','--lease-seconds',1,'--now',22.5));current=self.snapshot()
        self.queue('extend','job','worker',token,'--lease-seconds',2,'--now',22.75,code=2);self.assertEqual(self.snapshot(),current)
        new=self.queue('claim','other','--lease-seconds',2,'--now',22.75);self.assertNotEqual(new['lease_token'],token)
        after_reclaim=self.snapshot();self.queue('extend','job','worker',token,'--lease-seconds',20,'--now',23,code=2);self.assertEqual(self.snapshot(),after_reclaim)
    def test_invalid_requests_do_not_mutate(self):
        v=self.lease();token=v['lease_token'];before=self.snapshot()
        for id,worker,tok in [('missing','worker',token),('job','other',token),('job','worker','stale')]:
            self.queue('extend',id,worker,tok,'--lease-seconds',20,'--now',2,code=2);self.assertEqual(self.snapshot(),before)
        for field in ('--lease-seconds','--now'):
            for value in ('nan','inf','-inf','0','-1') if field=='--lease-seconds' else ('nan','inf','-inf'):
                args=['--lease-seconds',2,'--now',2];at=args.index(field);args[at+1:at+2]=[value];args[at:at+2]=[field+'='+value]
                self.queue('extend','job','worker',token,*args,code=2);self.assertEqual(self.snapshot(),before)
        for option in ('--now=nan','--lease-seconds=inf','--lease-seconds=0'):
            missing=self.directory/('fresh-'+str(len(option))+option[-3:]+'.sqlite')
            args=['--lease-seconds',1,'--now',0,option]
            self.queue('extend','job','worker',token,*args,database=missing,code=2);self.assertFalse(missing.exists())
        self.queue('ack','job','worker',token);before=self.snapshot();self.queue('extend','job','worker',token,'--lease-seconds',20,'--now',2,code=2);self.assertEqual(self.snapshot(),before)
    def test_negative_fractional_time_extension(self):
        self.queue('enqueue','negative','{}','--now=-10')
        lease=self.queue('claim','w','--lease-seconds',2.5,'--now=-5.5')
        result=self.queue('extend','negative','w',lease['lease_token'],'--lease-seconds',4.25,'--now=-4.5')
        self.assertEqual(result['lease_expires_at'],-.25)
        self.assertIsNone(self.queue('claim','other','--lease-seconds',1,'--now=-.5'))
    def test_concurrent_extensions_keep_maximum(self):
        token=self.lease()['lease_token'];values=self.race([['extend',self.database,'job','worker',token,'--lease-seconds',n,'--now',2] for n in (20,40)])
        self.assertTrue(all(code==0 for code,_,_ in values));self.assertEqual(self.rows()['job']['lease_expires_at'],42)

class Q3(QueueHelpers):
    def batch(self,items,code=0):
        p=self.directory/'batch.json';p.write_text(json.dumps(items,ensure_ascii=False));return self.queue('enqueue-batch',p,code=code)
    def test_insert_repeat_priority_and_counts(self):
        items=[{'task_id':'low','payload':[1,'雪']},{'task_id':'high','payload':None,'priority':5},{'task_id':'same','payload':{'a':1,'b':2},'priority':5,'max_attempts':2}]
        self.queue('enqueue','same','{"b":2,"a":1}','--priority',5,'--max-attempts',2,'--now',0)
        self.assertEqual(self.batch(items),{'created':2,'existing':1});self.assertEqual(self.batch(items),{'created':0,'existing':3})
        got=[]
        for _ in items:
            v=self.queue('claim','worker','--lease-seconds',10,'--now',0);got.append(v['task_id']);self.queue('ack',v['task_id'],'worker',v['lease_token'])
        self.assertEqual(got,['same','high','low']);self.assertEqual(json.loads(self.rows()['low']['payload']),[1,'雪'])
        self.assertEqual(self.batch([]),{'created':0,'existing':0})
    def test_all_validation_and_late_conflict_atomic(self):
        self.queue('enqueue','old','{}','--now',0);before=self.snapshot()
        bads=[[{'task_id':'new','payload':{}},{'task_id':'old','payload':1}],[{'task_id':'x','payload':{}},{'task_id':'x','payload':{}}],{},[{}],[{'task_id':'x'}],[{'task_id':'x','payload':{},'extra':1}],[{'task_id':'','payload':{}}],[{'task_id':'x','payload':{},'priority':'bad'}],[{'task_id':'x','payload':{},'max_attempts':0}],[{'task_id':'x','payload':{},'max_attempts':True}],[{'task_id':42,'payload':{}}],[{'task_id':'x','payload':{},'priority':True}],[{'task_id':'x','payload':{},'priority':1.5}],[{'task_id':'x','payload':float('nan')}],[{'task_id':'x','payload':float('inf')}]]
        for items in bads:
            with self.subTest(items=items):self.batch(items,code=2);self.assertEqual(self.snapshot(),before)
    def test_concurrent_identical_batches(self):
        self.queue('init');p=self.directory/'batch.json';p.write_text(json.dumps([{'task_id':'a','payload':1},{'task_id':'b','payload':[]}]))
        values=self.race([['enqueue-batch',self.database,p]]*2);self.assertTrue(all(x[0]==0 for x in values));self.assertEqual(sum(x[1]['created'] for x in values),2);self.assertEqual(sum(x[1]['existing'] for x in values),2);self.assertEqual(set(self.rows()),{'a','b'})

class Q4(QueueHelpers):
    def test_inspection_is_logically_read_only_and_unknown_null(self):
        self.populated();before=self.snapshot();rows=self.rows()
        for id in ('done','dead','queued','expired','active'):
            for now in (-.5,0,.5,1,20):
                row=rows[id];value=self.queue('inspect',id,'--now',now)
                self.assertEqual(value,{'task_id':id,'payload':json.loads(row['payload']),'stored_state':row['status'],'attempts':row['attempts'],'lease_expired':row['status']=='leased' and row['lease_expires_at']<=now})
                self.assertEqual(self.snapshot(),before)
        self.assertIsNone(self.queue('inspect','missing','--now',20));self.assertEqual(self.snapshot(),before)
        self.queue('stats','--now',20);self.assertEqual(self.rows()['expired']['status'],'queued')
    def test_invalid_time_and_missing_database_never_write(self):
        self.queue('init');before=self.snapshot()
        for value in ('nan','inf','-inf'):
            self.queue('inspect','missing','--now='+value,code=2);self.assertEqual(self.snapshot(),before)
        path=self.directory/'missing.sqlite';self.queue('inspect','x','--now',1,database=path,code=2);self.assertFalse(path.exists())

class F1(g.FlowCase):
    def marked(self,id,**kw):
        p=self.directory/id
        return self.task(id,command=self.python(f'from pathlib import Path; p=Path({str(p)!r}); p.write_text(p.read_text()+"x" if p.exists() else "x")'),**kw)
    def test_closure_duplicates_original_order_and_unrelated_not_run(self):
        tasks=[self.marked('leaf',depends_on=['left','right']),self.marked('unrelated'),self.marked('right',depends_on=['root']),self.marked('left',depends_on=['root']),self.marked('root')]
        r=self.flow(tasks,'--target','leaf','--target','left','--target','leaf','--jobs',3)
        self.assertEqual([x['id'] for x in r['tasks']],['leaf','right','left','root']);self.assertFalse((self.directory/'unrelated').exists())
        for id in ('leaf','left','right','root'):self.assertEqual((self.directory/id).read_text(),'x')
        r=self.flow(tasks);self.assertEqual([x['id'] for x in r['tasks']],[x['id'] for x in tasks]);self.assertTrue((self.directory/'unrelated').exists())
    def test_whole_plan_validated_and_target_failure_propagates(self):
        valid=self.marked('selected');self.output.write_bytes(b'prior\n')
        self.flow([valid],'--target','missing',code=2);self.assertFalse((self.directory/'selected').exists());self.assertEqual(self.output.read_bytes(),b'prior\n')
        self.flow([valid,self.task('invalid',depends_on=['absent'])],'--target','selected',code=2);self.assertFalse((self.directory/'selected').exists());self.assertEqual(self.output.read_bytes(),b'prior\n')
        r=self.flow([self.task('bad',command=self.python('raise SystemExit(7)'),retries=1),self.task('child',depends_on=['bad']),self.marked('other')],'--target','child',code=1)
        self.assertEqual([(x['id'],x['status'],x['attempts']) for x in r['tasks']],[('bad','failed',2),('child','skipped',0)]);self.assertFalse((self.directory/'other').exists())

class F2(g.FlowCase):
    def test_task_local_overlay_literal_and_retry(self):
        key='SCODE_BENCH_INHERITED';self.env[key]='parent-value';self.env['SCODE_BENCH_KEPT']='kept'
        literal='spaces " quotes $(touch SHOULD_NOT_EXIST); 雪'
        command=self.python('import os,json; print(json.dumps([os.environ.get("SCODE_BENCH_INHERITED"),os.environ.get("SCODE_BENCH_KEPT")]))')
        tasks=[self.task('a',command=command,env={key:literal}),self.task('b',command=command,env={key:'second'}),self.task('c',command=command),self.task('d',command=command,env={})]
        r=self.flow(tasks,'--jobs',3);self.assertEqual([json.loads(x['stdout']) for x in r['tasks']],[[literal,'kept'],['second','kept'],['parent-value','kept'],['parent-value','kept']]);self.assertEqual(self.env[key],'parent-value');self.assertFalse((Path(g.WORKSPACE)/'SHOULD_NOT_EXIST').exists())
        unusual=self.task('unusual',command=self.python('import os,json; print(json.dumps([os.environ.get("含 空格"),os.environ.get("9-label")]))'),env={'含 空格':'unicode-value','9-label':'digit-value'})
        self.assertEqual(json.loads(self.flow([unusual])['tasks'][0]['stdout']),['unicode-value','digit-value'])
        log=self.directory/'retry.txt';source=f'import os; from pathlib import Path; p=Path({str(log)!r}); p.write_text((p.read_text() if p.exists() else "")+os.environ[{key!r}]+"\\n"); raise SystemExit(7)'
        self.flow([self.task('r',command=self.python(source),env={key:'retry-value'},retries=1)],code=1);self.assertEqual(log.read_text().splitlines(),['retry-value']*2)
    def test_invalid_env_rejected_before_any_side_effect(self):
        marker=self.directory/'marker';good=self.task('first',command=self.python(f'from pathlib import Path; Path({str(marker)!r}).touch()'))
        for env in ([],None,{'': 'v'},{'a=b':'v'},{'x\0':'v'},{'x':1},{'x':None},{'x':'\0'}):
            self.output.write_bytes(b'prior\n');self.flow([good,self.task('bad',env=env)],'--jobs',3,code=2);self.assertFalse(marker.exists());self.assertEqual(self.output.read_bytes(),b'prior\n')

class F3(g.FlowCase):
    def test_nonzero_success_retries_default_and_observable_codes(self):
        tasks=[self.task('a',command=self.python('raise SystemExit(3)'),success_codes=[0,3],retries=2),self.task('b',depends_on=['a'])]
        r=self.flow(tasks);self.assertEqual((r['tasks'][0]['status'],r['tasks'][0]['attempts'],r['tasks'][0]['exit_code']),('succeeded',1,3));self.assertEqual(r['tasks'][1]['status'],'succeeded')
        del tasks[0]['success_codes'];r=self.flow(tasks,code=1);self.assertEqual((r['tasks'][0]['status'],r['tasks'][0]['attempts'],r['tasks'][1]['status']),('failed',3,'skipped'))
        for exitcode,allowed in [(7,[0,3]),(0,[3])]:
            r=self.flow([self.task('a',command=self.python(f'raise SystemExit({exitcode})'),success_codes=allowed,retries=1),self.task('b',depends_on=['a'])],code=1);self.assertEqual((r['tasks'][0]['exit_code'],r['tasks'][0]['attempts'],r['tasks'][1]['status']),(exitcode,2,'skipped'))
    def test_invalid_codes_rejected_before_commands(self):
        marker=self.directory/'marker';good=self.task('first',command=self.python(f'from pathlib import Path; Path({str(marker)!r}).touch()'))
        for codes in ([],[0,0],[-1],[256],[0.5],['0'],[True],None,0):
            self.output.write_bytes(b'prior\n');self.flow([good,self.task('bad',success_codes=codes)],'--jobs',3,code=2);self.assertFalse(marker.exists());self.assertEqual(self.output.read_bytes(),b'prior\n')
    def test_timeout_and_signal_are_not_success_codes(self):
        for command,kw in [(self.python('import time; time.sleep(2)'),{'timeout_seconds':.1}), (self.python('import os,signal; os.kill(os.getpid(),signal.SIGTERM)'),{})]:
            r=self.flow([self.task('a',command=command,success_codes=list(range(256)),**kw)],code=1);self.assertEqual(r['tasks'][0]['status'],'failed')

class F4(g.FlowCase):
    def preview(self,tasks,*args,code=0):
        self.plan.write_text(json.dumps({'tasks':tasks}));r=self.cli('flow_runner',self.plan,'--output',self.output,'--dry-run',*args,code=code)
        if code==0:self.assertTrue(r.stdout.endswith('\n'));return json.loads(r.stdout)
        return r
    def test_deterministic_layers_no_execution_or_publication(self):
        marker=self.directory/'started';cmd=self.python(f'from pathlib import Path; Path({str(marker)!r}).touch()')
        tasks=[self.task('end',command=cmd,depends_on=['right','left']),self.task('right',command=cmd,depends_on=['start'],resource='shared'),self.task('left',command=cmd,depends_on=['start'],resource='shared'),self.task('start',command=cmd)]
        self.output.write_bytes(b'prior\n');expected={'layers':[['start'],['left','right'],['end']]}
        self.assertEqual(self.preview(tasks,'--jobs',1),expected);self.assertEqual(self.preview(list(reversed(tasks)),'--jobs',8),expected);self.assertFalse(marker.exists());self.assertEqual(self.output.read_bytes(),b'prior\n')
        self.assertEqual(self.preview([]),{'layers':[]});self.assertEqual(self.output.read_bytes(),b'prior\n')
        self.flow(tasks);self.assertTrue(marker.exists());self.assertNotEqual(self.output.read_bytes(),b'prior\n')
    def test_invalid_plans_reject_preview(self):
        marker=self.directory/'invalid-started'
        worker=self.task('worker',command=self.python(f'from pathlib import Path; Path({str(marker)!r}).touch()'))
        for tasks in ([worker,self.task('bad',command='not-an-argv-array')],[self.task('a',depends_on=['b']),self.task('b',depends_on=['a'])],[self.task('a'),self.task('b',depends_on=['a','a'])],[self.task('a',depends_on=['unknown'])]):
            self.output.write_bytes(b'prior\n');r=self.preview(tasks,code=2);self.assertEqual(r.stdout,'');self.assertEqual(self.output.read_bytes(),b'prior\n');self.assertFalse(marker.exists())

TASKS={name:globals()[name] for name in [f'{p}{i}' for p in 'RQF' for i in range(1,5)]}

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--workspace',required=True,type=Path);parser.add_argument('--task',required=True,choices=sorted(TASKS));args=parser.parse_args()
    g.WORKSPACE=args.workspace.resolve();g.TASK=args.task
    if not g.WORKSPACE.is_dir() or Path(__file__).resolve().is_relative_to(g.WORKSPACE):parser.error('external grader must be outside candidate')
    train={'R':g.ReportTrain,'Q':g.QueueTrain,'F':g.FlowTrain}[args.task[0]]
    suite=unittest.TestSuite(unittest.defaultTestLoader.loadTestsFromTestCase(case) for case in (g.PublicBaseline,train,TASKS[args.task]))
    result=unittest.TextTestRunner(verbosity=2,stream=sys.stderr).run(suite)
    print(json.dumps({'task':args.task,'passed':result.wasSuccessful(),'checks':result.testsRun,'failures':len(result.failures),'errors':len(result.errors)},sort_keys=True))
    return 0 if result.wasSuccessful() else 1
if __name__=='__main__':raise SystemExit(main())
