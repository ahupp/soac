import json
import sys

sys.path.insert(0, "tmp")
from soac.import_hook import install

install()
import direct_constructor_failure_exception as module

print(json.dumps(module.run()))
