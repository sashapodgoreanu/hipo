# Internal Contract: Runner Resources Settings IPC

| Command | Input | Result |
|---|---|---|
| settings_get_runner_resources | workspace | requested/effective RunnerResourcesProfile e diagnostica non sensibile |
| settings_set_runner_resources | workspace + profilo completo | versione accettata, profilo effettivo e diagnostica |

Il frontend invia il profilo completo con una sola operazione.

1. Il profilo valido diventa subito desiderato.
2. Ready applica la generazione; starting non pubblica una generazione vecchia.
3. Leased drena query attive con il vecchio profilo; le nuove partono solo dopo apply atomico dell'ultima versione.
4. Save concorrenti coalescono.
5. Apply failure conserva il profilo effettivo e restituisce configuration_apply_failed alle nuove query fino a correzione.

Se il resolver non produce un profilo effettivo, il comando restituisce
`invalid_profile` e reason code sanitizzato; non avvia o modifica worker. I
campi di diagnostica ammessi sono versione richiesta/effettiva, clamp reason e
metriche aggregate; endpoint, PID, path, token, SQL e secret sono esclusi.

PipelineEvent resta compatibile; pool/profile events sono DTO additivi e non includono endpoint, credential, SQL, PID o path.
