# Examples

The public examples are a small, supported corpus: each one demonstrates a
library capability referenced by the documentation. They are intentionally
organized by task rather than by release history.

Build the extension first, then run an example from the repository root:

```bash
./build.sh
uv run python examples/basics/basic_navigation.py
```

Most examples launch a local Chrome instance. The deployment examples require
Chrome running with the repository's Docker configuration; examples that use
public sites may need a network connection and can change as those sites change.

## Basics

| Example | Demonstrates |
| --- | --- |
| [`basic_navigation.py`](basics/basic_navigation.py) | Open a pooled tab, navigate, and read page content. |
| [`dom_and_interaction.py`](basics/dom_and_interaction.py) | Query, type, and click DOM elements. |
| [`actions_demo.py`](basics/actions_demo.py) | Extract typed data with `Schema` and actions. |
| [`javascript_eval.py`](basics/javascript_eval.py) | Evaluate JavaScript in a rendered page. |
| [`multi_page.py`](basics/multi_page.py) | Work with several session pages concurrently. |
| [`screenshot_and_pdf.py`](basics/screenshot_and_pdf.py) | Capture a page PNG screenshot. |

## Pooling and lifecycle

| Example | Demonstrates |
| --- | --- |
| [`page_lifecycle.py`](pooling/page_lifecycle.py) | Blank pages, init scripts, and cancellation-safe cleanup. |
| [`response_capture.py`](pooling/response_capture.py) | Capture action-triggered responses without page-world hooks. |
| [`concurrent_resource_tabs.py`](pooling/concurrent_resource_tabs.py) | Reuse pooled tabs for concurrent resource extraction. |
| [`network_logging.py`](pooling/network_logging.py) | Observe network activity and inspect resource requests. |

## Configuration and profiles

| Example | Demonstrates |
| --- | --- |
| [`cookies.py`](configuration/cookies.py) | Set, inspect, and delete cookies through CDP. |
| [`custom_headers_and_proxy.py`](configuration/custom_headers_and_proxy.py) | Configure request headers and an upstream proxy. |
| [`connect_to_existing_chrome.py`](configuration/connect_to_existing_chrome.py) | Attach to an existing Chrome DevTools endpoint. |
| [`profile_split_headful.py`](configuration/profile_split_headful.py) | Fork a native Chrome-profile baseline for headful workers. |

## Deployment

| Example | Demonstrates |
| --- | --- |
| [`docker_headless.py`](deployment/docker_headless.py) | Connect a pool to headless Chrome in Docker. |
| [`docker_headful.py`](deployment/docker_headful.py) | Connect a pool to headful Docker Chrome with VNC/noVNC. |

## Advanced browser boundaries

| Example | Demonstrates |
| --- | --- |
| [`accessibility_navigation.py`](advanced/accessibility_navigation.py) | Locate and interact through Chrome's accessibility tree. |
| [`antibot_detection.py`](advanced/antibot_detection.py) | Classify anti-bot/CDN responses and active challenges. |
| [`cross_origin_iframe_eval.py`](advanced/cross_origin_iframe_eval.py) | Evaluate JavaScript in a cross-origin frame. |
| [`turnstile_checkbox_ax.py`](advanced/turnstile_checkbox_ax.py) | Reach a closed-shadow, cross-origin Turnstile checkbox through AX. |

## Local development

`examples/development/` is intentionally ignored by Git and Ruff. Create it
locally with `mkdir -p examples/development` for personal experiments,
target-specific scripts, and load tests that are not part of the supported
documentation corpus. Do not add documentation links to files in that directory.
