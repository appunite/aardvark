from . import echo
from . import numpy_case
from . import pandas_case

SCENARIOS = {
    "echo": echo.main,
    "numpy": numpy_case.main,
    "pandas": pandas_case.main,
}
