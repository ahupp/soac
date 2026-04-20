import sys

sys.path.insert(0, "scripts")

from soac.import_hook import install

install()
import soac.runtime

print("ok", soac.runtime.DELETED)
