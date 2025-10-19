.PHONY: pyodide-fetch perf-all perf-md setup-python

# Default to the curated Pyodide cache shipped with the repo (cp313 build).
PYODIDE_DIR ?= $(PWD)/tmp/pyodide
ITERATIONS ?= 25
PERF_JSON ?= target/perf/results.json
PERF_CSV ?= target/perf/results.csv
PERF_MD ?= target/perf/results.md
PYODIDE_VERSION ?= 0.28.2
PYODIDE_VARIANT ?= full

setup-python:
	@echo "Installing Python $(PYODIDE_VERSION) toolchain via mise"
	@mise install python@$(PYODIDE_VERSION)

pyodide-fetch:
	@echo "Fetching upstream Pyodide $(PYODIDE_VERSION) ($(PYODIDE_VARIANT)) into .aardvark/pyodide"
	@cargo run -p aardvark-cli --bin cargo-aardvark -- fetch-pyodide \
		--version $(PYODIDE_VERSION) --variant $(PYODIDE_VARIANT)

perf-all:
	@mkdir -p $(dir $(PERF_JSON))
	AARDVARK_PYODIDE_PACKAGE_DIR=$(PYODIDE_DIR) cargo run -p aardvark-perf -- \
		all --iterations $(ITERATIONS) --json $(PERF_JSON) --csv $(PERF_CSV)

perf-md: perf-all
	@mkdir -p $(dir $(PERF_MD))
	python perf/scripts/render_markdown.py $(PERF_JSON) > $(PERF_MD)
	@echo "Markdown written to $(PERF_MD)"
