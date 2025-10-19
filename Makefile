.PHONY: perf-all perf-md setup-python

# Default location for manually downloaded Pyodide assets.
PYODIDE_DIR ?= $(PWD)/.aardvark/pyodide/$(PYODIDE_VERSION)
ITERATIONS ?= 25
PERF_JSON ?= target/perf/results.json
PERF_CSV ?= target/perf/results.csv
PERF_MD ?= target/perf/results.md
PYODIDE_VERSION ?= 0.28.2

setup-python:
	@echo "Installing Python $(PYODIDE_VERSION) toolchain via mise"
	@mise install python@$(PYODIDE_VERSION)

perf-all:
	@mkdir -p $(dir $(PERF_JSON))
	AARDVARK_PYODIDE_PACKAGE_DIR=$(PYODIDE_DIR) cargo run -p aardvark-perf -- \
		all --iterations $(ITERATIONS) --json $(PERF_JSON) --csv $(PERF_CSV)

perf-md: perf-all
	@mkdir -p $(dir $(PERF_MD))
	python perf/scripts/render_markdown.py $(PERF_JSON) > $(PERF_MD)
	@echo "Markdown written to $(PERF_MD)"
