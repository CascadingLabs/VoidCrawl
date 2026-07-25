from pathlib import Path

ROOT = Path(__file__).parents[1]
WORKFLOWS = ROOT / ".github" / "workflows"


def test_standard_workflows_exclude_free_threaded_python() -> None:
    for workflow_name in ("CI.yaml", "release.yml"):
        workflow = (WORKFLOWS / workflow_name).read_text()
        assert "3.14t" not in workflow
        assert "cp314t" not in workflow


def test_sdist_packages_and_validates_vendored_chromiumoxide() -> None:
    pyproject = (ROOT / "pyproject.toml").read_text()
    ci_workflow = (WORKFLOWS / "CI.yaml").read_text()
    release_workflow = (WORKFLOWS / "release.yml").read_text()

    assert 'path = "vendor/chromiumoxide/**/*"' in pyproject
    assert "vendor/chromiumoxide/Cargo.toml" in ci_workflow
    assert "pip wheel --no-deps dist/*.tar.gz" in ci_workflow
    assert "vendor/chromiumoxide/Cargo.toml" in release_workflow


def test_free_threaded_canary_is_manual_and_does_not_publish() -> None:
    workflow = (WORKFLOWS / "free-threaded-canary.yml").read_text()

    assert "workflow_dispatch:" in workflow
    assert "3.14t" in workflow
    assert "cp314t" in workflow
    assert "gh-action-pypi-publish" not in workflow
