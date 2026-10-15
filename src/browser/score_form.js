// Se ejecuta en la página autenticada; los argumentos se serializan como JSON.
const find = control => {
    const elements = [...document.getElementsByName(control.name)];
    if (elements.length !== 1 || elements[0].id !== control.id) throw new Error('control');
    const element = elements[0];
    if (element.disabled || element.readOnly || !element.getClientRects().length) throw new Error('disabled');
    return element;
};
const editControl = plan => {
    const save = plan.form.save_control;
    if (!save.name.endsWith('$CtlSave') || !/_CtlSave(?:_\d+)?$/.test(save.id)) throw new Error('save');
    return {name: save.name.replace(/\$CtlSave$/, '$CtlEditar'), id: save.id.replace(/_CtlSave(?=_\d+$|$)/, '_CtlEditar')};
};
const panelFor = plan => {
    if (location.href !== plan.form.url) throw new Error('location');
    const control = document.getElementsByName(plan.form.save_control.name).length
        ? plan.form.save_control : editControl(plan);
    const panel = find(control).closest('div[id*=CtlWodsResults][id*=CtlUpMain]');
    let heading = panel?.previousElementSibling;
    while (heading && heading.tagName !== 'H4') heading = heading.previousElementSibling;
    if (!panel || heading?.textContent.trim() !== plan.wod_name.trim()) throw new Error('wod');
    return panel;
};
const scoped = (plan, control) => {
    const element = find(control);
    if (!panelFor(plan).contains(element)) throw new Error('scope');
    return element;
};
const armPostback = () => {
    document.documentElement.dataset.pocPostback = 'pending';
    const manager = window.Sys?.WebForms?.PageRequestManager?.getInstance();
    if (manager) {
        const done = (_, args) => {
            document.documentElement.dataset.pocPostback = args.get_error() ? 'error' : 'done';
            manager.remove_endRequest(done);
        };
        manager.add_endRequest(done);
    }
};
const validate = plan => {
    const save = scoped(plan, plan.form.save_control);
    const options = save.getAttribute('onclick')?.match(/new WebForm_PostBackOptions\(([^)]*)\)/);
    if (!options) return 'ready';
    const [target, , causesValidation, group] = JSON.parse(`[${options[1]}]`);
    if (target !== save.name) throw new Error('validation-target');
    if (!causesValidation) return 'ready';
    if (typeof window.Page_ClientValidate !== 'function') throw new Error('validation-unavailable');
    if (window.Page_ClientValidate(group)) return 'ready';
    const errors = [...(window.Page_Validators || [])]
        .filter(v => v.validationGroup === group && v.isvalid === false)
        .map(v => {
            const text = document.createElement('div');
            text.innerHTML = v.errormessage || 'Campo inválido';
            return text.textContent.trim();
        });
    return `invalid: ${errors.join('; ') || 'El formulario no supera la validación de Arena'}`;
};
const run = async request => {
    const {plan, step, operation} = request;
    if (operation === 'wait') {
        if (document.documentElement.dataset.pocPostback === 'error') return 'error';
        return document.readyState === 'complete'
            && document.documentElement.dataset.pocPostback !== 'pending'
            && !window.Sys?.WebForms?.PageRequestManager?.getInstance()?.get_isInAsyncPostBack()
            ? 'ready' : 'pending';
    }
    panelFor(plan);
    if (operation === 'check') return 'ready';
    if (operation === 'open-edit') {
        if (document.getElementsByName(plan.form.save_control.name).length) return 'ready';
        const edit = scoped(plan, editControl(plan));
        if (edit.tagName !== 'INPUT' || edit.type !== 'submit') throw new Error('input');
        armPostback();
        edit.click();
        return 'pending';
    }
    if (operation === 'read-saved') {
        scoped(plan, editControl(plan));
        const results = panelFor(plan).querySelectorAll('.athleteresult .athlete .affiliate');
        if (results.length !== 1 || !results[0].textContent.trim()) throw new Error('saved-result');
        return 'saved:' + results[0].textContent.trim();
    }
    if (operation === 'validate') return validate(plan);
    if (operation === 'verify') {
        const matches = plan.form.steps.every(step => {
            const actual = scoped(plan, step.control).value.trim();
            if (step.role === 'weight') return actual !== '' && Number(actual.replace(',', '.')) === Number(step.value);
            if (step.role === 'time' || step.role === 'tie_break') {
                // La máscara de Arena vuelve a insertar __:__ al vaciar el desempate.
                // Solo equivale a vacío para este campo opcional; 00:00 sigue siendo un valor.
                if (step.role === 'tie_break' && step.value === '') return actual === '' || actual === '__:__';
                const seconds = value => /^\d+:\d{2}$/.test(value)
                    ? Number(value.split(':')[0]) * 60 + Number(value.split(':')[1]) : value;
                return seconds(actual) === seconds(step.value);
            }
            return actual === step.value;
        });
        return matches ? 'ready' : 'mismatch';
    }
    if (operation === 'save') {
        const save = scoped(plan, plan.form.save_control);
        if (save.tagName !== 'INPUT' || save.type !== 'submit') throw new Error('save');
        const validity = validate(plan);
        if (validity !== 'ready') return validity;
        armPostback();
        save.click();
        return 'pending';
    }
    const element = scoped(plan, step.control);
    if (step.action === 'select') {
        if (element.tagName !== 'SELECT' || ![...element.options].some(o => o.value === step.value && !o.disabled)) throw new Error('option');
        if (element.value === step.value) return 'ready';
        if (step.wait_for_postback) armPostback();
        element.value = step.value;
        element.dispatchEvent(new Event('change', {bubbles: true}));
        return step.wait_for_postback ? 'pending' : 'ready';
    }
    if (element.tagName !== 'INPUT' || !['text', 'number', 'tel'].includes(element.type)) throw new Error('input');
    element.focus();
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
    setter.call(element, step.value);
    element.dispatchEvent(new Event('input', {bubbles: true}));
    element.dispatchEvent(new Event('change', {bubbles: true}));
    // jQuery Masked Input procesa input/focus con setTimeout. Si salimos antes,
    // su tarea pendiente vuelve a insertar la máscara después del blur.
    await new Promise(resolve => setTimeout(resolve, 25));
    element.blur();
    await new Promise(resolve => setTimeout(resolve, 0));
    if (!element.checkValidity()) throw new Error('invalid');
    return 'ready';
};
return run(request);
