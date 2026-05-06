.PHONY: deps install uninstall lint format typecheck test coverage all

deps:
	uv sync --all-groups
	uv run pre-commit install --hook-type pre-commit --hook-type pre-push

install:
	uv tool install .

uninstall:
	uv tool uninstall sodagun

lint:
	uv run ruff check sodagun tests

format:
	uv run ruff format sodagun tests

typecheck:
	uv run mypy sodagun

test:
	uv run pytest

coverage:
	uv run pytest --cov=sodagun --cov-report=term-missing

all: format lint typecheck test
