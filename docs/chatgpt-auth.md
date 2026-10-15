# ChatGPT: proveedor nativo y autenticación OAuth

El PoC y el worker usan el proveedor `rig::providers::chatgpt` de Rig 0.44,
con `gpt-6-luna` por defecto, configurable mediante `CHATGPT_MODEL` en el entorno
o `.env`. El razonamiento es `xhigh`. La extracción de imágenes usa Responses y
`Extractor<ScorecardReading>` directamente desde Rust.

## Crear el archivo de autenticación

Ejecuta una vez:

```sh
cargo run --example chatgpt_login
```

Rig muestra una URL y un código de dispositivo. Abre la URL, introduce el código
y completa el inicio de sesión con ChatGPT. Al terminar, Rig guarda el archivo en
`$HOME/.config/wodbuster-jueces/auth.json`. El comando no necesita Postgres,
Browserless, una imagen ni llamadas al modelo.

Para elegir otro archivo, configura `CHATGPT_AUTH_FILE` en el entorno o `.env`:

```sh
export CHATGPT_AUTH_FILE=/ruta/privada/chatgpt-auth.json
cargo run --example chatgpt_login
cargo run
```

Usa el mismo valor de `CHATGPT_AUTH_FILE` para el login, el PoC y el servidor.
Un valor vacío utiliza la ruta predeterminada. El archivo tiene el formato nativo
de Rig: utiliza una ruta propia para esta aplicación.

## Reutilizar y renovar la sesión

Después del login:

```sh
cargo run
cargo run --example poc_agent -- examples/poc_agent.png tmp/poc-context.json
```

La aplicación usa `Authenticator::new(AuthSource::OAuth, Some(path), ..., false)`.
Rig lee la sesión y renueva y guarda los tokens cuando es necesario. La aplicación
no convierte formatos, decodifica JWT ni modifica las credenciales por su cuenta.

El comando de login utiliza el mismo autenticador con `allow_device_flow=true`,
sin el límite de 120 segundos de la extracción. El worker mantiene desactivado el
login interactivo: si necesita una nueva sesión, devuelve un error que indica
volver a ejecutar `cargo run --example chatgpt_login`.

En servicios o contenedores conserva el archivo en almacenamiento persistente,
con lectura y escritura para el usuario del servicio. Contiene credenciales:
mantenlo privado y fuera del repositorio, la base de datos y los logs.

Las fotos nuevas registran `provider=chatgpt` y el modelo usado. Refrescar vuelve a
analizar con el proveedor actual; corregir manualmente conserva la procedencia de
la lectura original.

Referencia de la implementación: `rig-core-0.44.0/src/providers/chatgpt/auth/`
y sus helpers de `providers/internal/auth/device.rs`.
