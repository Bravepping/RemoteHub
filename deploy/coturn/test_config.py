import contextlib,io,pathlib,tempfile,unittest
from render import render
from verify_config import HERE,check

class ConfigTests(unittest.TestCase):
    def test_blank_env_template_cannot_be_deployed(self):
        check((HERE/'turnserver.conf.example').read_text(encoding='utf-8'),True)
        with tempfile.TemporaryDirectory() as d:
            output=pathlib.Path(d)/'server.conf'
            with self.assertRaises(ValueError):render(HERE/'.env.example',output)
            self.assertFalse(output.exists())
    def test_render_no_overwrite_secret_safe_errors_and_policy_mutations(self):
        # Public IP fixtures validate data only; these tests perform no network access.
        with tempfile.TemporaryDirectory() as d:
            folder=pathlib.Path(d);env=folder/'private.env';out=folder/'server.conf'
            env.write_text('TURN_LISTEN_IP=10.0.0.2\nTURN_RELAY_IP=10.0.0.2\nTURN_REALM=fixture.invalid\nTURN_SHARED_SECRET='+('a'*64)+'\nTURN_PUBLIC_IP=8.8.8.8\nTURN_NAT_PRIVATE_IP=10.0.0.2\n',encoding='utf-8');env.chmod(0o600)
            render(env,out);text=out.read_text();check(text)
            with self.assertRaises(ValueError):render(env,out)
            for bad in [text+'no-auth\n',text+'allowed-peer-ip=127.0.0.1\n',text.replace('no-cli\n',''),text.replace('total-quota=8','total-quota=0'),text.replace('no-stdout-log\n',''),text.replace('denied-peer-ip=10.0.0.0-10.255.255.255\n',''),text.replace('listening-port=3478','listening-port=0'),text.replace('external-ip=8.8.8.8/10.0.0.2','external-ip=8.8.8.8/10.0.0.3')]:
                with self.assertRaises(ValueError):check(bad)
            with self.assertRaises(ValueError):check(text.replace('a'*64,'short'))
            out2=folder/'other.conf';env.write_text(env.read_text().replace('TURN_REALM=fixture.invalid','TURN_REALM=$(whoami)'),encoding='utf-8')
            with self.assertRaises(ValueError):render(env,out2)
            self.assertFalse(out2.exists())
    def test_error_cli_never_echoes_secret_payload(self):
        import subprocess,sys
        with tempfile.TemporaryDirectory() as d:
            p=pathlib.Path(d)/'bad.conf';p.write_text('static-auth-secret=DO_NOT_PRINT_THIS_VALUE\nno-auth\n',encoding='utf-8')
            r=subprocess.run([sys.executable,str(HERE/'verify_config.py'),'--config',str(p)],capture_output=True,text=True)
            self.assertEqual(r.returncode,1);self.assertNotIn('DO_NOT_PRINT_THIS_VALUE',r.stdout+r.stderr)

if __name__=='__main__':unittest.main()
