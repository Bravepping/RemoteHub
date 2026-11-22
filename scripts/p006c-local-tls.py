"""Generate a short-lived loopback-only test certificate; no cloud credentials."""
import argparse
import datetime
import ipaddress
import json
import secrets
import sys
from pathlib import Path
ap=argparse.ArgumentParser(description=__doc__)
ap.add_argument('--dir',type=Path,required=True);ap.add_argument('--port',type=int,required=True);ap.add_argument('--deps',type=Path)
a=ap.parse_args()
if a.deps:sys.path.insert(0,str(a.deps.resolve()))
from cryptography import x509
from cryptography.x509.oid import NameOID
from cryptography.hazmat.primitives import hashes,serialization
from cryptography.hazmat.primitives.asymmetric import ec
assert 1024<=a.port<=65535 and a.dir.is_dir()
key=ec.generate_private_key(ec.SECP256R1());name=x509.Name([x509.NameAttribute(NameOID.COMMON_NAME,'RemoteHub local synthetic test')])
now=datetime.datetime.now(datetime.timezone.utc)
certificate=(x509.CertificateBuilder().subject_name(name).issuer_name(name).public_key(key.public_key()).serial_number(x509.random_serial_number()).not_valid_before(now-datetime.timedelta(minutes=1)).not_valid_after(now+datetime.timedelta(hours=1)).add_extension(x509.SubjectAlternativeName([x509.IPAddress(ipaddress.ip_address('127.0.0.1'))]),critical=False).add_extension(x509.BasicConstraints(ca=False,path_length=None),critical=True).sign(key,hashes.SHA256()))
cert=certificate.public_bytes(serialization.Encoding.PEM).decode()
for name,content in [('cert.pem',cert),('key.pem',key.private_bytes(serialization.Encoding.PEM,serialization.PrivateFormat.PKCS8,serialization.NoEncryption()).decode())]:
 p=a.dir/name;assert not p.exists();p.write_text(content)
sender=secrets.token_hex(32);receiver=secrets.token_hex(32);expires=int(now.timestamp())+540
server=dict(listen=f'127.0.0.1:{a.port}',public_ipv4='8.8.8.8',sender_invitation=sender,receiver_invitation=receiver,invitation_expires_at=expires,realm='local.remotehub.invalid',certificate=str((a.dir/'cert.pem').resolve()),private_key=str((a.dir/'key.pem').resolve()))
client=dict(server_ipv4='127.0.0.1',signaling_port=a.port,ca_pem=cert,sender_invitation=sender,receiver_invitation=receiver,invitation_expires_at=expires)
for name,data in [('server-private.json',server),('client-private.json',client)]:
 p=a.dir/name;assert not p.exists();p.write_text(json.dumps(data))
print('Created ephemeral loopback TLS fixture; secrets not printed')
