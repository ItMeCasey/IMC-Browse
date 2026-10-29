"""Signs a file with the Ed25519 private key in the IMC_UPDATE_KEY environment variable (PEM text)
and writes the signature as hex. Used by the release workflow:  python tools/sign.py update.json update.json.sig"""
import os
import sys

from cryptography.hazmat.primitives.serialization import load_pem_private_key

key = load_pem_private_key(os.environ["IMC_UPDATE_KEY"].encode(), password=None)
data = open(sys.argv[1], "rb").read()
open(sys.argv[2], "w").write(key.sign(data).hex())
print("signed", sys.argv[1])
