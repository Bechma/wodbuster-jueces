(() => {
  const notice = document.querySelector('[data-load-status]');
  if (!notice || notice.dataset.loadState !== 'running') return;
  const message = notice.querySelector('[data-load-message]');
  const buttons = document.querySelectorAll('[data-load-trigger]');
  buttons.forEach(button => { button.disabled = true; });
  function action(url, label) {
    const form = document.createElement('form');
    form.method = 'get';
    const target = new URL(url, window.location.href);
    form.action = target.pathname;
    target.searchParams.forEach((value, name) => {
      const input = document.createElement('input');
      input.type = 'hidden';
      input.name = name;
      input.value = value;
      form.append(input);
    });
    form.dataset.loadAction = '';
    const button = document.createElement('button');
    button.type = 'submit';
    button.className = 'secondary';
    button.textContent = label;
    form.append(button);
    notice.append(form);
  }
  async function check() {
    try {
      const response = await fetch(notice.dataset.loadStatus, {
        credentials: 'same-origin', cache: 'no-store', headers: { Accept: 'application/json' }
      });
      if (response.status === 401) {
        notice.className = 'notice error load-notice';
        notice.setAttribute('aria-busy', 'false');
        message.textContent = 'La sesión ha caducado. Vuelve a iniciar sesión.';
        action('/?error=session', 'Iniciar sesión');
        return;
      }
      if (!response.ok) throw new Error('status unavailable');
      const job = await response.json();
      message.textContent = job.message;
      notice.dataset.loadState = job.state;
      if (job.state !== 'running') {
        notice.setAttribute('aria-busy', 'false');
        notice.className = `notice ${job.state === 'succeeded' ? 'success' : 'error'} load-notice`;
        if (job.state === 'succeeded') action(job.refresh_url, 'Actualizar página');
        else {
          buttons.forEach(button => { button.disabled = false; });
          if (job.error_code === 'session') action('/?error=session', 'Iniciar sesión');
        }
        return;
      }
    } catch {
      message.textContent = 'No se pudo comprobar el estado. Se volverá a intentar.';
    }
    window.setTimeout(check, 1500);
  }
  check();
})();
