from pathlib import Path
import sys


ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT))

from tests._integration import render_transformed_source


print(render_transformed_source(Path("tests/integration_modules/for_else_continue_minimal.py")))
