.PHONY: help perf-all perf-md setup-python

.DEFAULT_GOAL := help

# Default location for manually downloaded Pyodide assets.
PYODIDE_DIR ?= $(PWD)/.aardvark/pyodide/$(PYODIDE_VERSION)
ITERATIONS ?= 25
PERF_JSON ?= target/perf/results.json
PERF_CSV ?= target/perf/results.csv
PERF_MD ?= target/perf/results.md
PYODIDE_VERSION ?= 0.28.2
PYTHON_VERSION ?= 3.13

help:
	@printf "Available targets:\n"
	@printf "  make perf-all     Run the full perf suite (JSON/CSV artefacts).\n"
	@printf "  make perf-md      Generate Markdown summary (runs perf-all first).\n"
	@printf "  make setup-python Install Python %s via mise (used by host runner).\n" "$(PYTHON_VERSION)"
	@printf "Variables:\n"
	@printf "  PYODIDE_VERSION=%s\n" "$(PYODIDE_VERSION)"
	@printf "  PYODIDE_DIR=%s\n" "$(PYODIDE_DIR)"
	@printf "  ITERATIONS=%s\n" "$(ITERATIONS)"

setup-python:
	@echo "Installing Python $(PYTHON_VERSION) toolchain via mise"
	@mise install python@$(PYTHON_VERSION)

perf-all:
	@mkdir -p $(dir $(PERF_JSON))
	AARDVARK_PYODIDE_PACKAGE_DIR=$(PYODIDE_DIR) cargo run -p aardvark-perf -- \
		all --iterations $(ITERATIONS) --json $(PERF_JSON) --csv $(PERF_CSV)

perf-md: perf-all
	@mkdir -p $(dir $(PERF_MD))
	python perf/scripts/render_markdown.py $(PERF_JSON) > $(PERF_MD)
	@echo "Markdown written to $(PERF_MD)"
