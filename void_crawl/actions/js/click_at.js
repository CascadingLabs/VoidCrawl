const el = document.elementFromPoint(__params.x, __params.y);
if (el) {
    el.dispatchEvent(new MouseEvent('click', {
        bubbles: true,
        cancelable: true,
        clientX: __params.x,
        clientY: __params.y,
        view: window
    }));
}
return el ? el.tagName.toLowerCase() : null;
