# Fotos de puntuaciones

En `/competitions/{id}`, **📷 Tomar foto** abre una vista previa de la cámara.
**Elegir foto** permite seleccionar una imagen o utilizar la captura nativa del dispositivo.
La cámara en directo requiere HTTPS o localhost y permiso del navegador.

La imagen se orienta y comprime en el dispositivo antes de subirla: máximo 1920 píxeles
en el lado mayor, WebP con calidad 0,82 (0,75 si supera 350 KB), con JPEG como alternativa.
No se amplían imágenes pequeñas ni se reduce agresivamente la resolución para alcanzar
un tamaño arbitrario. El máximo admitido es 1,5 MB; el servidor comprueba el formato real,
las dimensiones y que la imagen se pueda decodificar. La foto comprimida se guarda en
Postgres, sin duplicar el original de la cámara.

**Ver fotos** abre `/competitions/{id}/photos`. Muestra la foto, fecha, tamaño,
proveedor/modelo, estado, campos extraídos y posibles errores. El estado se actualiza
automáticamente. Las imágenes y resultados requieren la sesión de la cuenta propietaria.

Cada foto tiene un botón pequeño **↻** que vuelve a encolarla desde el análisis de IA,
con el modelo configurado actualmente; al obtener datos válidos se guarda otra vez en Arena.
Al pulsarlo aparece inmediatamente **Solicitando nuevo análisis…**, junto al encabezado
de la foto. El icono gira mientras se solicita o procesa el trabajo, y el aviso va mostrando
cola, análisis, preparación, guardado y resultado final. Si falla la solicitud o la consulta
de estado, el aviso queda visible. Perder la conexión para consultar no repite el envío.
Los campos aparecen como texto en una tabla, cada uno con un botón **Editar** que abre
su control. **Cerrar** oculta el control y conserva la corrección pendiente. Categoría,
equipo y WOD se seleccionan del contexto guardado; tiempo, repeticiones, kg y desempate
dependen del formato del WOD. Guardar permanece desactivado hasta que exista un cambio
real. Las filas modificadas se marcan en naranja; restaurar el valor original quita
la marca y, si no hay otros cambios, desactiva Guardar.
**Guardar cambios y encolar** valida y persiste la lectura corregida y el plan del formulario,
y encola directamente el envío a Arena, sin repetir el análisis de IA. También se pueden
corregir lecturas incompletas. Se conserva el modelo que produjo la lectura original.

Las dos acciones requieren CSRF y pertenencia a la cuenta/competición. Mientras una foto
está en cola, analizándose o enviándose, sus acciones permanecen bloqueadas. Una revisión
comparada atómicamente impide que dos peticiones o una pestaña antigua sobrescriban otra
edición. Las escrituras del worker también comprueban la revisión y el estado reclamados,
por lo que un trabajo antiguo no puede sobrescribir una corrección o un nuevo análisis.
La consulta automática de estados conserva los cambios sin guardar del formulario.

## Procesamiento

El servidor inicia un worker con `photos::start`. Un advisory lock de Postgres, mantenido
en una conexión dedicada, permite un solo worker global por base de datos, incluso si
se inicia más de una instancia de la aplicación. Procesa una foto cada vez:

1. `queued` → `parsing`: obtiene el contexto de la competición de la cuenta propietaria.
2. Llama al mismo extractor de Rig/ChatGPT que el PoC, en `src/ai.rs`, con el proveedor nativo de Rig.
3. Guarda la lectura, la preparación y los errores de validación, conservando el proveedor
   y modelo elegidos al subir la imagen. Si faltan datos queda en `incomplete`.
4. `parsed` → `submitting`: vuelve a validar contra el contexto actual y reutiliza
   `ArenaBrowser::submit_score`. Espera si los navegadores de la aplicación están ocupados.
5. Abre **Edit** cuando corresponde, rellena y guarda únicamente el WOD seleccionado.
   Relee la vista de juez para verificar el resultado y actualiza la puntuación local.
6. Persiste primero la confirmación remota manteniendo `submitting` y las acciones
   bloqueadas. Actualiza la puntuación local y termina en `succeeded` en una misma
   transacción; si falla esa copia, termina en `saved_local_failed`.

Al cargar los detalles, los WODs ya guardados se abren con **Edit** para capturar sus
campos y el control real de **Guardar**. La exploración de formatos nunca guarda
puntuaciones y conserva los valores originales. Así, refrescar los detalles permite
seguir preparando correcciones y nuevos envíos de puntuaciones existentes. Los
snapshots anteriores que solo contienen **Edit** necesitan refrescarse una vez.

Si `needs_retake` es `true`, pero los campos son suficientes y válidos, continúa el envío.
La galería muestra un aviso cuando Arena confirma el guardado de esa lectura.

`failed` conserva el error de extracción o de un envío que no llegó a guardar.
`unconfirmed` indica que hay que comprobar Arena antes de repetir el envío.
`saved_local_failed` conserva la confirmación remota si falta actualizar la caché local.
Un reinicio reanuda el análisis interrumpido e invalida la revisión del worker anterior.
Un envío interrumpido se marca como `unconfirmed` si no hay confirmación persistida,
o `saved_local_failed` si Arena ya lo confirmó; ninguno se repite automáticamente. Una petición de subida repetida con el
mismo UUID no duplica la foto ni el trabajo.

Se utiliza la sesión OAuth creada con `cargo run --example chatgpt_login`, guardada en
`$HOME/.config/wodbuster-jueces/auth.json` o en `CHATGPT_AUTH_FILE`, y `CHATGPT_MODEL`, con
`gpt-6-luna` por defecto. Consulta [autenticación de ChatGPT](chatgpt-auth.md) para preparar
otras ejecuciones o servicios. Postgres, Browserless y la cookie cifrada son los mismos
que usa la aplicación. El usuario debe haber cargado los detalles de la competición.

## Pruebas

```sh
cargo test --all-targets
cargo test --lib photos::tests -- --include-ignored
cargo test --test photos_web -- --ignored
cargo test --lib regression_tests -- --ignored
```

Las pruebas de integración requieren los servicios de `compose.dev.yml` y `DATABASE_URL`.
La cámara se simula en la prueba de Browserless, que comprueba la compresión y el cierre
de sus pistas. La prueba opcional siguiente llama a ChatGPT y **guarda realmente** la
puntuación de la imagen del PoC en Arena:

```sh
cargo test --test live_photos -- --ignored --nocapture
```

Referencias: [captura de imágenes](https://developer.mozilla.org/en-US/docs/Web/HTML/Reference/Attributes/capture),
[compresión con canvas](https://developer.mozilla.org/en-US/docs/Web/API/HTMLCanvasElement/toBlob),
[advisory locks de Postgres](https://www.postgresql.org/docs/current/explicit-locking.html#ADVISORY-LOCKS).
