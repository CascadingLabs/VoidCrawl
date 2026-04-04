const el = document.querySelector(__params.selector);
if (!el) throw new Error('Element not found: ' + __params.selector);
const rect = el.getBoundingClientRect();
const cx = rect.left + rect.width / 2;
const cy = rect.top + rect.height / 2;
el.dispatchEvent(new MouseEvent('mouseenter', {
    bubbles: false, clientX: cx, clientY: cy, view: window
}));
el.dispatchEvent(new MouseEvent('mouseover', {
    bubbles: true, clientX: cx, clientY: cy, view: window
}));
return el.tagName.toLowerCase();
