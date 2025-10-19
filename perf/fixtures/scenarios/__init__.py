def _echo():
    from . import echo

    return echo.main


def _numpy():
    from . import numpy_case

    return numpy_case.main


def _pandas():
    from . import pandas_case

    return pandas_case.main


SCENARIOS = {
    "echo": _echo,
    "numpy": _numpy,
    "pandas": _pandas,
}
