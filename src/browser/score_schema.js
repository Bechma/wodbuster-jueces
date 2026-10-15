// Observe only captured Edit controls and score modes; never click Save.
const {control, wod_name, value} = request;
const matches = [...document.getElementsByName(control.name)];
if (matches.length !== 1 || matches[0].id !== control.id) return 'invalid';
const element = matches[0];
if (element.disabled || element.readOnly || !element.getClientRects().length) return 'invalid';
const panel = element.closest('div[id*=CtlWodsResults][id*=CtlUpMain]');
let heading = panel?.previousElementSibling;
while (heading && heading.tagName !== 'H4') heading = heading.previousElementSibling;
if (!panel || heading?.textContent.trim() !== wod_name.trim()) return 'invalid';
if (value === null) {
    if (!control.name.endsWith('$CtlEditar') || element.tagName !== 'INPUT' || element.type !== 'submit') return 'invalid';
} else {
    if (!control.name.endsWith('$IoComplete') || element.tagName !== 'SELECT'
        || ![...element.options].some(option => option.value === value && !option.disabled)) return 'invalid';
    if (element.value === value) return 'ready';
}
document.documentElement.dataset.scoreSchemaPending = 'true';
const manager = window.Sys?.WebForms?.PageRequestManager?.getInstance();
if (manager) {
    const done = (_, args) => {
        if (!args.get_error()) document.documentElement.removeAttribute('data-score-schema-pending');
        manager.remove_endRequest(done);
    };
    manager.add_endRequest(done);
}
if (value === null) element.click();
else {
    element.value = value;
    element.dispatchEvent(new Event('change', {bubbles: true}));
}
return 'pending';
