---
name: memoria-global
description: Guarda en tu memoria global 3D (cgm, solo tuya, compartida por todas tus sesiones y proyectos) lo que valga la pena recordar, delegando la escritura en un subagente de un modelo pequeño. Úsala cuando el usuario te corrija o te enseñe cómo trabajar, cuando se tome una decisión de diseño, al terminar un trabajo real en un proyecto, cuando aprendas algo técnico que costó encontrar, o cuando el usuario diga «recuerda», «guárdalo» o «memoria». Una vez por momento así; nunca en bucle ni de forma programada.
---

# Memoria global

Es tu memoria entre sesiones. Leerla no cuesta nada: al iniciar la sesión y con cada mensaje, los
hooks te inyectan lo que aplica, en un bloque «Memoria global» que trae el **id de la sesión** y cada
recuerdo con su `#id`. La escribe un subagente barato, y solo cuando tú se lo pides.

## Cuándo guardar

Guarda al cerrar un momento que valga la pena, no en cada mensaje:

| Momento | Qué guardar | `donde` / `tipo` |
|---|---|---|
| El usuario te corrigió o te dijo cómo quiere que trabajes | una lección sobre ti | `donde: yo` |
| Se decidió algo de diseño | la decisión y su porqué | `tipo: decision`, en el proyecto |
| Terminaste un trabajo real | cómo quedó y qué falta, más un resumen | resumen con `tipo: sesion` |
| Encontraste algo técnico que costó (una API, un fallo silencioso) | el hallazgo | `donde: oficio:<tema>` |
| Quieres retomar algo tú mismo más adelante | lo pendiente | `tipo: plan` |
| El usuario lo pide | lo que pida | — |

El uso cuesta: **una llamada al subagente por momento**. Nada de guardados programados ni de hooks
que lancen modelos solos.

## Cómo: con un subagente pequeño

Lanza la herramienta `Agent` con el modelo más pequeño disponible (por ejemplo `model: "haiku"`) y
este encargo. Sustituye `<ID>` por el id de la sesión, que viene en la cabecera de la memoria inyectada:

```text
Eres quien escribe la memoria global de un agente (solo suya). Pasos:
1. Ejecuta: cgm sesion --id <ID>
   Es lo dicho en la sesión desde lo último guardado. Si dice «nada nuevo», termina.
2. Decide qué merece recordarse en sesiones futuras, como máximo 8 entradas. Solo lo duradero y no
   obvio:
   - lecciones y correcciones del usuario;
   - sus preferencias;
   - decisiones con su causa;
   - el estado en que queda un proyecto;
   - hallazgos técnicos difíciles;
   - planes del agente;
   - y SIEMPRE un resumen de la sesión (tipo "sesion", 1-2 frases: qué se hizo y cómo queda).
   Nunca contraseñas, tokens ni claves. Nada que se pueda leer tal cual en el código o en git.
   Escribe frases completas que se entiendan solas, en el idioma del usuario.
3. Escribe la lista JSON con la herramienta Write en un archivo temporal (p. ej. cgm-<ID>.json):
   [{"texto": "...", "tipo": "regla|decision|hecho|incidente|convencion|plan|comando|sesion",
     "donde": "yo|proyecto|carpeta|persona:<Nombre>|oficio:<tema>|<ruta de carpeta>",
     "origen": "usuario (si lo dijo el usuario) | agente (si lo concluyó el agente)",
     "causa": "por qué (opcional)", "sustituye": <#id viejo que esto reemplaza, opcional>,
     "nucleo": true solo para lecciones de identidad que deban estar en toda sesión}]
4. Ejecuta: cgm anotar --sesion <ID> "<ese archivo>"
5. Responde solo con las líneas que imprimió cgm anotar.
No ejecutes otros comandos ni toques otros archivos.
```

Cuando vuelva, revisa en una línea qué guardó. Si algo está mal, corrígelo con `cgm negar <id>`.

## Leer y mantener (sin modelo)

- `cgm recordar "<texto>"`: busca cuando la memoria inyectada no basta.
- `cgm agenda` / `cgm hecho <id>`: tus planes pendientes, y marcarlos como cumplidos.
- `cgm confirmar <id>` / `cgm negar <id>`: cuando el usuario confirma o desmiente algo.
- `cgm nucleo <id> [--quitar]`: lo que debe estar en toda sesión, o dejar de estarlo.
- `cgm lista`: todo, con su χ.

## Límites

- Es **tuya**. No copies tu contexto personal a otros agentes, ni el suyo a esta memoria. Una memoria
  común entre agentes sería otra, aparte.
- Lo que concluyes tú nace con evidencia neutra; lo que dice el usuario nace con más. No subas a mano
  la certeza de lo tuyo.
