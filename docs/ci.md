# CI y publicación de la imagen

Hay dos workflows:

- `.github/workflows/ci.yml` se ejecuta en los pushes de ramas, pull requests y
  manualmente. Comprueba `cargo fmt`, Clippy con avisos como errores, las pruebas
  unitarias y ejemplos, y las integraciones locales con Postgres y Browserless.
  Utiliza Rust stable y caché de Cargo con `Swatinem/rust-cache`. Las integraciones
  se seleccionan explícitamente para evitar acceder a Arena o usar la IA real.
- `.github/workflows/publish.yml` se ejecuta al terminar **CI**. Solo publica si
  terminó correctamente para un push del propio repositorio a su rama principal
  configurada en GitHub. Hace checkout del commit que pasó las pruebas y construye
  la aplicación con `cargo build --release --locked` dentro de Docker.

La imagen Linux amd64 se publica como:

```text
ghcr.io/bechma/wodbuster-jueces:latest
ghcr.io/bechma/wodbuster-jueces:sha-<commit completo>
```

Cada commit validado obtiene su etiqueta `sha-…`. `latest` se actualiza cuando ese
commit sigue siendo la cabeza de la rama principal al preparar la publicación;
una ejecución antigua no sustituye la imagen de una revisión más reciente.
GitHub proporciona `GITHUB_TOKEN`, con `packages: write` en el job de publicación;
no se necesita guardar un PAT. La etiqueta OCI `source` vincula el paquete al repositorio.
El workflow de publicación debe estar en la rama principal para recibir `workflow_run`.

El Dockerfile separa las dependencias con `cargo-chef` para reutilizar su compilación
cuando cambia el código. Buildx restaura y exporta las capas mediante caché de GitHub
Actions (`type=gha`, `mode=max`). Los servicios y las imágenes base usan `:latest`;
la publicación utiliza `pull: true` para resolver las imágenes base actuales.

## Imagen y configuración

La imagen ejecuta el binario como usuario `10001:10001`, escucha en `0.0.0.0:3000`
y contiene las migraciones y recursos de la aplicación integrados en el binario.
`.dockerignore` limita el contexto a Cargo, el código y las migraciones: las claves,
sesiones OAuth, `.env`, fotos locales y archivos de pruebas quedan fuera de la imagen.

La configuración se proporciona al iniciar el contenedor: `DATABASE_URL`,
`BROWSER_URL`, `BROWSER_TOKEN`, `SESSION_SECRET` y las opciones de `.env.example`.
Postgres y Browserless se ejecutan como servicios externos. En una red Docker,
sus URLs deben usar el nombre DNS del servicio correspondiente.

La sesión de ChatGPT se monta en un directorio persistente, con lectura y escritura
para el usuario del contenedor. Consulta [autenticación de ChatGPT](chatgpt-auth.md).
El archivo predeterminado dentro de la imagen es
`/home/app/.config/wodbuster-jueces/auth.json`; `CHATGPT_AUTH_FILE` permite cambiarlo.

Para comprobar la construcción localmente:

```sh
docker build --pull -t wodbuster-jueces:local .
```

Referencias: [evento workflow_run](https://docs.github.com/en/actions/reference/workflows-and-actions/events-that-trigger-workflows#workflow_run),
[caché de Docker en GitHub Actions](https://docs.docker.com/build/ci/github-actions/cache/),
[cargo-chef](https://github.com/LukeMathWalker/cargo-chef).
