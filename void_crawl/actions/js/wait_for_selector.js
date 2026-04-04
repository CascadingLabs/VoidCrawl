const deadline = Date.now() + (__params.timeout * 1000);
while (Date.now() < deadline) {
    const el = document.querySelector(__params.selector);
    if (el) return el.tagName.toLowerCase();
    await new Promise(r => setTimeout(r, 100));
}
throw new Error('Timeout waiting for: ' + __params.selector);
