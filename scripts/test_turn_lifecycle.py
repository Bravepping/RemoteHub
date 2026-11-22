"""Offline fixtures only, never described as actual cloud evidence."""
import unittest
from turn_lifecycle_analyze import classify
from turn_lifecycle_observer import lifecycle_events
from g6_turn_wire import packet, parse
import struct
import tempfile
from pathlib import Path
from turn_lifecycle_observer import read_capture


def fixture(zero=False, gone=61, end=80):
    events = [dict(event='socket_snapshot',timeNs=0,servicePid=123,relayPorts=[]),dict(event='turn_wire', case='fixture', kind='0x103', lifetime=60, timeNs=10**9, relayPort=49160, transactionHash='allocate')]
    for second in range(1, end):
        events.append(dict(event='socket_snapshot',timeNs=second*10**9,servicePid=123,relayPorts=[49160] if gone is None or second<gone else []))
    if zero:
        events += [dict(event='turn_wire',case='fixture',kind='0x4',lifetime=0,timeNs=2*10**9,transactionHash='zero'),dict(event='turn_wire',case='fixture',kind='0x104',lifetime=0,timeNs=2*10**9+10**7,transactionHash='zero')]
    return sorted(events,key=lambda e:e['timeNs'])


class LifecycleTests(unittest.TestCase):
    def test_shared_username_preserves_each_client_allocation_and_refresh(self):
        packets=[]
        server=('192.0.2.1',3478)
        for i,client in enumerate([('192.0.2.2',40001),('192.0.2.2',40002)],1):
            for kind,tid,attrs,src,dst in [
                (3,bytes([i])*12,[(6,b'shared-session')],client,server),
                (0x103,bytes([i])*12,[(13,struct.pack('!I',600))],server,client),
                (4,bytes([i+2])*12,[(6,b'shared-session'),(13,struct.pack('!I',0))],client,server),
                (0x104,bytes([i+2])*12,[(13,struct.pack('!I',0))],server,client),
            ]:
                k,t,v=parse(packet(kind,tid,attrs));packets.append((len(packets)*10**9,k,t,v,src,dst))
        rows,_=lifecycle_events(packets,{'shared-session':'fixture'},True)
        self.assertEqual({r['case'] for r in rows},{'fixture.allocation-1','fixture.allocation-2'})
        for case in {'fixture.allocation-1','fixture.allocation-2'}:
            selected=[r for r in rows if r['case']==case]
            self.assertEqual([r['kind'] for r in selected],['0x3','0x103','0x4','0x104'])
        text=str(rows)
        self.assertNotIn('shared-session',text);self.assertNotIn('192.0.2',text)

    def test_refresh_zero_needs_response_and_socket_disappearance(self):
        self.assertEqual(classify(fixture(True,3),'fixture')['classification'],'refresh_zero_immediate')
        missing=[e for e in fixture(True,3) if e.get('kind')!='0x104']
        self.assertEqual(classify(missing,'fixture')['classification'],'inconclusive')
        late = [e for e in fixture(True,61) if e.get('kind')!='0x104']
        self.assertEqual(classify(late,'fixture')['classification'],'lease_timeout')
        self.assertEqual(classify(fixture(True,None),'fixture')['classification'],'abnormal_residual')

    def test_timeout_uses_granted_not_requested_lifetime(self):
        self.assertEqual(classify(fixture(),'fixture')['classification'],'lease_timeout')
        events=fixture(gone=None,end=75);next(e for e in events if e.get('kind')=='0x103')['lifetime']=600
        self.assertEqual(classify(events,'fixture')['classification'],'pending')

    def test_service_stop_or_restart_cannot_prove_release(self):
        events=fixture(gone=2)
        self.assertEqual(classify(events,'fixture')['classification'],'inconclusive')
        events[-1]['servicePid']=456
        self.assertEqual(classify(events,'fixture')['classification'],'inconclusive')

    def test_renewal_updates_deadline_and_residual_is_explicit(self):
        events=fixture(gone=None,end=100)
        events.append(dict(event='turn_wire',case='fixture',kind='0x104',lifetime=60,timeNs=40*10**9,transactionHash='renew'))
        events.sort(key=lambda e:e['timeNs'])
        self.assertEqual(classify(events,'fixture')['classification'],'pending')
        self.assertEqual(classify(fixture(gone=None,end=80),'fixture')['classification'],'abnormal_residual')

    def test_parser_never_exports_username_or_address(self):
        tid=bytes(range(12));user='private-user';src=('192.0.2.2',12345);dst=('192.0.2.1',3478)
        kind,tx,attrs=parse(packet(3,tid,[(6,user.encode()),(13,struct.pack('!I',60))]))
        rows,_=lifecycle_events([(10**9,kind,tx,attrs,src,dst)],{user:'fixture'})
        text=str(rows)
        self.assertNotIn(user,text);self.assertNotIn(src[0],text);self.assertNotIn(dst[0],text)
        self.assertEqual(rows[0]['lifetime'],60)

    def test_actual_pcap_layout_and_partial_record_are_parsed(self):
        stun=packet(3,bytes(range(12)),[(6,b'private-user'),(13,struct.pack('!I',60))])
        ip=bytearray(20);ip[0]=0x45;ip[9]=17;ip[12:16]=bytes([192,0,2,2]);ip[16:20]=bytes([192,0,2,1])
        frame=bytes(20)+ip+struct.pack('!HHHH',12345,3478,8+len(stun),0)+stun
        header=bytes.fromhex('d4c3b2a1')+struct.pack('<HHIIII',2,4,0,0,1024,276)
        record=struct.pack('<IIII',100,123456,len(frame),len(frame))+frame
        with tempfile.TemporaryDirectory() as folder:
            p=Path(folder)/'fixture.pcap';p.write_bytes(header+record+record[:8])
            rows=read_capture(p)
        self.assertEqual(len(rows),1);self.assertEqual(rows[0][0],100123456000)
        self.assertEqual(rows[0][1],3)

    def test_capture_duplicate_is_not_counted_as_two_allocations(self):
        payload=packet(3,bytes(12),[(6,b'private-user')]);kind,tid,values=parse(payload)
        item=(10**9,kind,tid,values,('192.0.2.2',12345),('192.0.2.1',3478))
        rows,_=lifecycle_events([item,item],{'private-user':'fixture'})
        self.assertEqual(len(rows),1)


if __name__ == '__main__':
    unittest.main()
