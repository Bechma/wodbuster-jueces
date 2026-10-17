(() => {
    const captureId = () => {
        if (crypto.randomUUID) return crypto.randomUUID();
        const bytes = crypto.getRandomValues(new Uint8Array(16));
        bytes[6] = (bytes[6] & 15) | 64; bytes[8] = (bytes[8] & 63) | 128;
        const hex = [...bytes].map(b => b.toString(16).padStart(2, '0')).join('');
        return `${hex.slice(0,8)}-${hex.slice(8,12)}-${hex.slice(12,16)}-${hex.slice(16,20)}-${hex.slice(20)}`;
    };
    const encode = (canvas, quality) => new Promise((resolve, reject) => {
        canvas.toBlob(blob => {
            if (!blob) return reject(new Error('No se pudo comprimir la foto.'));
            if (blob.type === 'image/webp') return resolve(blob);
            canvas.toBlob(jpeg => jpeg ? resolve(jpeg) : reject(new Error('No se pudo comprimir la foto.')), 'image/jpeg', quality);
        }, 'image/webp', quality);
    });
    async function optimize(source, width, height) {
        const scale = Math.min(1, 1920 / Math.max(width, height));
        const canvas = document.createElement('canvas');
        canvas.width = Math.max(1, Math.round(width * scale));
        canvas.height = Math.max(1, Math.round(height * scale));
        const context = canvas.getContext('2d');
        context.fillStyle = 'white'; context.fillRect(0, 0, canvas.width, canvas.height);
        context.drawImage(source, 0, 0, canvas.width, canvas.height);
        let blob = await encode(canvas, 0.82);
        if (blob.size > 350 * 1024) blob = await encode(canvas, 0.75);
        if (blob.size > 1.5 * 1024 * 1024) throw new Error('La foto es demasiado grande. Acerca la cámara a la hoja y vuelve a intentarlo.');
        return blob;
    }
    async function compress(file) {
        const url = URL.createObjectURL(file);
        const image = new Image();
        try {
            image.src = url;
            await image.decode();
            return await optimize(image, image.naturalWidth, image.naturalHeight);
        } finally { URL.revokeObjectURL(url); }
    }
    const useNativeCamera = navigator.userAgentData?.mobile
        || /Android|iPhone|iPad|iPod/i.test(navigator.userAgent)
        || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
    for (const form of document.querySelectorAll('[data-photo-upload]')) {
        const input = form.querySelector('[data-photo-input]');
        const cameraInput = form.querySelector('[data-photo-camera-input]');
        const button = form.querySelector('[data-photo-open]');
        const choose = form.querySelector('[data-photo-file]');
        const dialog = form.querySelector('dialog');
        const video = dialog.querySelector('video');
        const shutter = dialog.querySelector('[data-photo-capture]');
        let stream = null;
        const stopCamera = () => { stream?.getTracks().forEach(track => track.stop()); stream = null; video.srcObject = null; };
        dialog.addEventListener('close', stopCamera);
        window.addEventListener('pagehide', stopCamera);
        dialog.querySelector('[data-photo-cancel]').addEventListener('click', () => dialog.close());
        choose.addEventListener('click', () => input.click());
        const message = form.querySelector('[data-photo-message]');
        const captureLabel = button.textContent;
        const expectedResults = form.dataset.expectedResults || '1';
        let capture = null;
        let uploading = false;
        button.addEventListener('click', async () => {
            if (capture) return upload();
            if (useNativeCamera || !navigator.mediaDevices?.getUserMedia || !dialog.showModal) return cameraInput.click();
            shutter.disabled = true;
            dialog.querySelector('[data-camera-message]').textContent = 'Abriendo cámara…';
            dialog.showModal();
            try {
                const camera = await navigator.mediaDevices.getUserMedia({audio: false, video: {facingMode: {ideal: 'environment'}, width: {ideal: 1920}, height: {ideal: 1080}}});
                if (!dialog.open) { camera.getTracks().forEach(track => track.stop()); return; }
                stream = camera; video.srcObject = stream;
                await video.play();
                shutter.disabled = false;
                dialog.querySelector('[data-camera-message]').textContent = 'Encuadra la hoja para que los nombres y la puntuación se lean con claridad.';
            } catch (_) {
                stopCamera();
                dialog.querySelector('[data-camera-message]').textContent = 'No se pudo abrir la cámara. Comprueba los permisos o utiliza «Galería o archivos». La cámara requiere HTTPS o localhost.';
            }
        });
        shutter.addEventListener('click', async () => {
            if (!video.videoWidth || uploading) return;
            shutter.disabled = true;
            try {
                const blob = await optimize(video, video.videoWidth, video.videoHeight);
                capture = {id: captureId(), blob};
                dialog.close();
                await upload();
            } catch (error) { dialog.querySelector('[data-camera-message]').textContent = error.message; }
            finally { shutter.disabled = false; }
        });
        for (const fileInput of [input, cameraInput]) fileInput.addEventListener('change', async () => {
            if (!fileInput.files[0] || uploading) return;
            button.disabled = true; choose.disabled = true;
            message.textContent = 'Reduciendo el tamaño de la foto…';
            try {
                capture = {id: captureId(), blob: await compress(fileInput.files[0])};
                await upload();
            } catch (error) { message.textContent = error.message; }
            finally { fileInput.value = ''; button.disabled = false; choose.disabled = false; }
        });
        async function upload() {
            if (uploading || !capture) return;
            uploading = true; button.disabled = true; choose.disabled = true;
            message.textContent = 'Guardando foto…';
            try {
                const body = new FormData();
                body.append('csrf', form.querySelector('[name=csrf]').value);
                body.append('capture_id', capture.id);
                body.append('expected_results', expectedResults);
                body.append('photo', capture.blob, 'puntuacion.' + (capture.blob.type === 'image/webp' ? 'webp' : 'jpg'));
                const response = await fetch(form.action, {method: 'POST', body, credentials: 'same-origin'});
                if (!response.ok) throw new Error(response.status === 401 ? 'La sesión ha caducado. Inicia sesión de nuevo.' : 'No se pudo subir la foto. Comprueba que la competición tenga sus detalles cargados.');
                await response.json();
                message.textContent = 'Foto guardada. La IA y el guardado en Wodbuster continúan en segundo plano. Puedes consultar «Ver fotos». ' + Math.round(capture.blob.size / 1024) + ' KB.';
                capture = null; button.textContent = captureLabel;
            } catch (error) {
                message.textContent = error.message;
                button.textContent = 'Reintentar subida'; // Reutiliza el UUID para evitar duplicar el trabajo.
            } finally { uploading = false; button.disabled = false; choose.disabled = false; }
        }
    }
    const gallery = document.querySelector('[data-photo-gallery]');
    if (!gallery) return;
    const categories = JSON.parse(gallery.dataset.photoContext || '[]');
    const cards = new Map();
    let actionGeneration = 0;
    const timeText = time => time ? `${String(time.minutes).padStart(2, '0')}:${String(time.seconds).padStart(2, '0')}` : '';
    const choices = (select, items, value, placeholder) => {
        select.replaceChildren(new Option(placeholder, ''));
        for (const [id, name] of items) select.add(new Option(name, String(id)));
        if (value != null && value !== '' && ![...select.options].some(o => o.value === String(value))) {
            select.add(new Option(`Referencia no disponible (${value})`, String(value)));
        }
        select.value = value == null ? '' : String(value);
    };
    const kindNames = {completed: 'Completado: Sí', not_completed: 'Completado: No', time: 'Tiempo', repetitions: 'Repeticiones', weight: 'Peso (kg)'};
    const allowedKinds = kind => ({time_or_repetitions: ['completed', 'not_completed'], time: ['time'], repetitions: ['repetitions'], weight: ['weight']}[kind] || Object.keys(kindNames));
    for (const card of gallery.querySelectorAll('[data-photo-id]')) {
        const edit = card.querySelector('[data-photo-edit]');
        const retry = card.querySelector('[data-photo-refresh]');
        const deleteForm = card.querySelector('[data-photo-delete]');
        const deleteDialog = card.querySelector('[data-photo-delete-dialog]');
        const deleteOpen = card.querySelector('[data-photo-delete-open]');
        const field = name => edit.elements.namedItem(name);
        const item = {card, edit, retry, field, dirty: false, pending: false, signature: null, busy: edit.querySelector('fieldset').disabled, baseline: {}, open: new Set(), state: card.dataset.photoStatus};
        item.tracking = item.busy;
        const message = card.querySelector('[data-photo-action-message]');
        item.feedback = (text, tone = 'loading') => {
            if (message.textContent !== text) message.textContent = text;
            message.className = `notice photo-progress ${tone}`;
            message.hidden = false;
        };
        const names = [...edit.querySelectorAll('[data-field-value]')].map(value => value.dataset.fieldValue);
        const save = edit.querySelector('[data-photo-save]');
        item.lock = () => {
            edit.querySelector('fieldset').disabled = item.busy || item.pending;
            retry.querySelector('button').disabled = item.busy || item.pending;
            deleteOpen.disabled = item.busy || item.pending;
            deleteForm.querySelector('[type=submit]').disabled = item.busy || item.pending;
            save.disabled = item.busy || item.pending || !item.dirty;
            retry.querySelector('button').setAttribute('aria-busy', String(item.busy || item.pending));
            retry.querySelector('[data-photo-refresh-icon]').classList.toggle('is-processing', item.busy || item.pending);
        };
        item.display = () => {
            item.dirty = false;
            for (const name of names) {
                const control = field(name);
                const changed = control.value !== item.baseline[name];
                item.dirty ||= changed;
                edit.querySelector(`[data-photo-field="${name}"]`).classList.toggle('is-modified', changed);
                edit.querySelector(`[data-field-value="${name}"]`).textContent = control.value === '' ? 'Sin datos' :
                    (control.tagName === 'SELECT' ? control.selectedOptions[0]?.textContent : control.value);
                edit.querySelector(`[data-field-control="${name}"]`).hidden = !item.open.has(name);
                const button = edit.querySelector(`[data-field-edit="${name}"]`);
                button.textContent = item.open.has(name) ? 'Cerrar' : 'Editar';
                button.setAttribute('aria-expanded', String(item.open.has(name)));
            }
            item.lock();
        };
        item.snapshot = () => {
            item.baseline = Object.fromEntries(names.map(name => [name, field(name).value]));
            item.display();
        };
        for (const button of edit.querySelectorAll('[data-field-edit]')) button.addEventListener('click', () => {
            const name = button.dataset.fieldEdit;
            if (item.open.has(name)) item.open.delete(name); else item.open.add(name);
            item.display();
            if (item.open.has(name)) field(name).focus();
        });
        // La validación nativa puede detectar datos ausentes en una fila cerrada.
        edit.addEventListener('invalid', event => {
            item.open.add(event.target.name);
            item.display();
        }, true);
        cards.set(card.dataset.photoId, item);
        deleteOpen.addEventListener('click', () => {
            if (item.busy || item.pending) return;
            deleteForm.querySelector('[data-photo-delete-error]').hidden = true;
            deleteDialog.showModal();
        });
        card.querySelector('[data-photo-delete-cancel]').addEventListener('click', () => deleteDialog.close());
        deleteForm.addEventListener('submit', async event => {
            event.preventDefault();
            if (!deleteDialog.open || item.busy || item.pending) return;
            const body = new URLSearchParams(new FormData(deleteForm));
            item.pending = true; item.lock(); actionGeneration++;
            try {
                const response = await fetch(deleteForm.action, {method:'POST', body, credentials:'same-origin'});
                const result = await response.json().catch(() => null);
                if (!response.ok || !result?.deleted) throw new Error(result?.message || 'No se pudo borrar la foto. Actualiza la vista e inténtalo otra vez.');
                deleteDialog.close(); card.remove(); cards.delete(card.dataset.photoId);
                const feedback = gallery.querySelector('[data-photo-list-feedback]');
                feedback.textContent = result.message; feedback.hidden = false;
                gallery.querySelector('[data-photo-empty]').hidden = cards.size > 0;
            } catch (error) {
                const message = deleteForm.querySelector('[data-photo-delete-error]');
                message.textContent = error.message; message.hidden = false;
            } finally { item.pending = false; item.lock(); actionGeneration++; await refresh(); }
        });
        const selectedCategory = () => categories.find(c => String(c.id) === field('category_id').value);
        const selectedWod = (suffix = '') => selectedCategory()?.wods.find(w => String(w.id) === field('wod_id' + suffix).value);
        function scoreFields(suffix = '') {
            const active = suffix === '' || field('results_count').value === '2';
            const kind = field('kind' + suffix).value;
            if (suffix) for (const name of ['wod_id', 'kind', 'time', 'repetitions', 'kilograms', 'tie_break']) {
                card.querySelector(`[data-photo-field="${name}${suffix}"]`).hidden = !active;
                field(name + suffix).disabled = !active;
                field(name + suffix).required = active && ['wod_id', 'kind'].includes(name);
                if (!active) item.open.delete(name + suffix);
            }
            const visible = {
                time: active && ['completed', 'time'].includes(kind),
                repetitions: active && ['not_completed', 'repetitions'].includes(kind),
                kilograms: active && kind === 'weight',
                tie_break: active && !!selectedWod(suffix)?.tie_break && ['completed', 'not_completed'].includes(kind)
            };
            for (const [name, show] of Object.entries(visible)) {
                card.querySelector(`[data-score-field="${name}${suffix}"]`).hidden = !show;
                field(name + suffix).disabled = !show;
                field(name + suffix).required = show && name !== 'tie_break';
                if (!show) item.open.delete(name + suffix);
            }
        }
        function wodOptions(value, suffix = '') {
            choices(field('kind' + suffix), allowedKinds(selectedWod(suffix)?.kind).map(k => [k, kindNames[k]]), value, 'Selecciona el resultado');
            scoreFields(suffix);
        }
        function categoryOptions(reading) {
            const category = selectedCategory();
            choices(field('team_reference'), (category?.teams || []).map(t => [t.reference, `${t.name} · ${t.reference}`]), reading.team_reference, 'Selecciona el equipo');
            const results = reading.results || [reading];
            for (const [index, suffix] of ['', '_2'].entries()) {
                choices(field('wod_id' + suffix), (category?.wods || []).map(w => [w.id, `${w.name} · ${w.id}`]), results[index]?.wod_id, 'Selecciona el WOD');
                wodOptions(results[index]?.score_value?.kind, suffix);
            }
        }
        item.populate = reading => {
            reading ||= {};
            edit.querySelector('[data-photo-judge]').textContent = reading.judge_name?.trim() || 'No detectado';
            const results = reading.results || [reading];
            field('results_count').value = String(reading.results?.length || (reading.wod_id !== undefined ? 1 : card.dataset.expectedResults || 1));
            choices(field('category_id'), categories.map(c => [c.id, `${c.name} · ${c.id}`]), reading.category_id, 'Selecciona la categoría');
            categoryOptions(reading);
            for (const [index, suffix] of ['', '_2'].entries()) {
                const value = results[index]?.score_value;
                field('time' + suffix).value = timeText(value?.time);
                field('repetitions' + suffix).value = value?.repetitions ?? '';
                field('kilograms' + suffix).value = value?.kilograms ?? '';
                field('tie_break' + suffix).value = timeText(value?.tie_break);
            }
            field('needs_retake').value = String(!!reading.needs_retake);
            field('retake_reason').value = reading.retake_reason || '';
            item.snapshot();
        };
        const initial = JSON.parse(card.dataset.photoReading || 'null');
        item.populate(initial);
        item.signature = JSON.stringify(initial);
        field('category_id').addEventListener('change', () => categoryOptions({}));
        for (const suffix of ['', '_2']) {
            field('wod_id' + suffix).addEventListener('change', () => wodOptions(field('kind' + suffix).value, suffix));
            field('kind' + suffix).addEventListener('change', () => scoreFields(suffix));
        }
        field('results_count').addEventListener('change', () => scoreFields('_2'));
        edit.addEventListener('input', item.display);
        edit.addEventListener('change', item.display);
        for (const form of [edit, retry]) form.addEventListener('submit', async event => {
            event.preventDefault();
            if (item.pending || item.busy) return;
            if (form === edit && !item.dirty) return;
            if (form === retry && item.dirty) {
                item.feedback('Hay cambios sin guardar. Guarda los cambios o recarga la página antes de volver a analizar la foto.', 'warning');
                return;
            }
            const body = new URLSearchParams(new FormData(form));
            const previousLabel = card.querySelector('[data-photo-state]').textContent;
            item.pending = true; item.lock(); actionGeneration++;
            card.querySelector('[data-photo-state]').textContent = form === retry ? 'Solicitando nuevo análisis…' : 'Encolando puntuación…';
            item.feedback(form === retry ? 'Solicitando nuevo análisis de la imagen…' : 'Guardando cambios y encolando la puntuación…');
            card.querySelector('[data-photo-error]').hidden = true;
            card.querySelector('[data-photo-warning]').hidden = true;
            try {
                const response = await fetch(form.action, {method: 'POST', body, credentials: 'same-origin'});
                const result = await response.json().catch(() => null);
                if (!response.ok) throw new Error(result?.message || (response.status === 401 ? 'La sesión ha caducado. Inicia sesión de nuevo.' : 'No se pudo encolar. Recarga la página e inténtalo otra vez.'));
                item.busy = true; item.open.clear(); item.snapshot();
                item.tracking = true;
                item.state = form === retry ? 'queued' : 'parsed';
                card.querySelector('[data-photo-state]').textContent = form === retry ? 'En cola' : 'Pendiente de guardar';
                item.feedback(result.message);
            } catch (error) {
                card.querySelector('[data-photo-state]').textContent = previousLabel;
                item.feedback(error.message, 'error');
            }
            finally { item.pending = false; item.lock(); actionGeneration++; await refresh(); }
        });
        item.lock();
    }
    async function refresh() {
        const generation = actionGeneration;
        try {
            const response = await fetch(gallery.dataset.photoGallery, {credentials: 'same-origin', cache: 'no-store'});
            if (!response.ok) throw new Error(response.status === 401 ?
                'La sesión ha caducado. Inicia sesión de nuevo para consultar el estado.' :
                'No se pudo consultar el estado. Volveremos a intentarlo automáticamente.');
            const photos = await response.json();
            if (generation !== actionGeneration) return;
            if (gallery.dataset.photoFiltered === 'true') {
                const assigned = new Set(photos.map(photo => photo.id));
                let changed = photos.some(photo => !cards.has(photo.id));
                for (const [id, item] of cards) {
                    if (assigned.has(id)) continue;
                    changed = true;
                    // Conserva una edición que aún no se ha enviado.
                    if (!item.pending && !item.dirty) { item.card.remove(); cards.delete(id); }
                }
                if (changed) gallery.querySelector('[data-photo-list-changed]').hidden = false;
                gallery.querySelector('[data-photo-empty]').hidden = cards.size > 0 || photos.length > 0;
            }
            for (const photo of photos) {
                const item = cards.get(photo.id);
                if (!item || item.pending) continue;
                const {card, edit, retry} = item;
                if (item.tracking || photo.state !== item.state) {
                    item.feedback(photo.progress, photo.busy ? 'loading' :
                        (photo.state === 'succeeded' ? 'success' : 'warning'));
                }
                item.state = photo.state;
                item.tracking = photo.busy;
                card.querySelector('[data-photo-state]').textContent = photo.label;
                card.querySelector('[data-photo-target]').textContent = photo.target;
                card.querySelector('[data-photo-results]').textContent = photo.result_summary;
                card.dataset.expectedResults = String(photo.expected_results);
                card.querySelector('[data-photo-model]').textContent = `Modelo: ${photo.provider} / ${photo.model}`;
                const sync = card.querySelector('[data-photo-sync]');
                sync.textContent = photo.sync_label;
                sync.className = `notice ${photo.sync_status === 'synced' ? 'success' : 'warning'}`;
                card.querySelector('[data-photo-sync-time]').textContent = photo.sync_checked_at ? `Datos de Arena: ${photo.sync_checked_at}` : '';
                const duplicate = card.querySelector('[data-photo-duplicate]');
                duplicate.textContent = photo.duplicate_label;
                duplicate.hidden = photo.duplicate_count < 2;
                const signature = JSON.stringify(photo.reading);
                if (!item.dirty) {
                    if (signature !== item.signature) item.populate(photo.reading);
                    item.signature = signature;
                    for (const form of [edit, retry, card.querySelector('[data-photo-delete]')]) form.elements.namedItem('revision').value = photo.revision;
                }
                item.busy = photo.busy; item.lock();
                card.querySelector('[data-photo-warning]').hidden = !photo.warning;
                const error = card.querySelector('[data-photo-error]');
                error.textContent = photo.error || photo.validation_errors.join(' · ');
                error.hidden = !error.textContent;
            }
        } catch (error) {
            if (generation !== actionGeneration) return;
            for (const item of cards.values()) if (item.busy && !item.pending) {
                item.feedback(error.message === 'Failed to fetch' ?
                    'Se ha perdido la conexión para consultar el estado. El trabajo puede seguir en el servidor; volveremos a consultar automáticamente.' : error.message, 'warning');
            }
        }
    }
    async function poll() {
        await refresh();
        window.setTimeout(poll, 3000);
    }
    window.setTimeout(poll, 1500);
})();
