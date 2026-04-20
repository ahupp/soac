import sys

sys.path.insert(0, "tmp")
from soac.import_hook import install

install()
import field_method_case as module

assert module.run() == 8
