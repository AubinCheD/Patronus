# Patronus — feuille de route

État au 21/10/2026, version 0.1.0. Le premier jet (commits `3fcaccc` à `60e002b`) est livré : tâches
en fichiers, déclencheurs heure, intervalle et fichier modifié, actions script et session Claude non
interactive, boîte de résultats, interface. Ce document fixe ce qui suit : la passe de relecture (A),
les points remontés après le premier jet (B) et les fonctionnalités ajoutées (C). Il sert de
spécification au plan `docs/superpowers/plans/2026-10-21-passe-complete.md`. Les décisions sont dans
`journal.md`, la vision dans `spec.md`.

## Sommaire

| # | Point | État |
|---|---|---|
| A1 | Boutons des lignes de tâches qui reçoivent leurs clics ; tests d'interface | fait |
| A2 | Tâche qui écrit dans le dossier qu'elle surveille : plus de boucle | fait |
| A3 | Moteur dans son propre fil, arrêt propre | fait |
| A4 | Consigne par l'entrée standard, `claude.exe` ou `claude.cmd`, listes plafonnées | fait |
| A5 | Tours et budget du vrai Claude Code ; faux exécutable conforme | fait |
| A6 | Isolation des sessions : worktree pour écrire, mode restreint, push impossible | fait |
| A7 | Processus enfants dans un objet de tâche Windows | fait |
| A8 | Fenêtre, ligne de commande et autres processus sur le même dossier | fait |
| A9 | Rétention appliquée ; worktrees nettoyés et réconciliés | fait |
| A10 | Interface sobre ; brouillon de l'éditeur protégé | fait |
| A11 | Chemins non ASCII, sortie de cmd, problèmes sans doublons | fait |
| A12 | Formats : champs inconnus à tous les niveaux, états de référence | à faire |
| A13 | Échéances : veille, longues absences, tâche nouvelle, changement d'heure | à faire |
| B1 | Symboles de l'interface : un glyphe pour chacun, vérifié | à faire |
| B2 | Déclencheur « branche distante » | à faire |
| B3 | Application fermée : tâches du Planificateur de Windows | à faire |
| B4 | Premier essai réel de l'action Claude | préparé, par l'utilisateur |
| C1 | Actions sur un résultat : appliquer le diff, worktree, reprendre la session | à faire |
| C2 | Modèles de tâches | à faire |
| C3 | Budget mensuel des sessions Claude | à faire |
| C4 | Démarrage avec Windows, fermeture pendant une exécution | à faire |
| C5 | Résumé et liens écrits par le script | à faire |
| C6 | Petits plus : pause générale, recherche, dupliquer, zoom, icône | à faire |

## A. Passe de relecture

Relecture complète du premier jet par un relecteur indépendant (21/10), confrontée à la documentation de
Claude Code et à la CLI installée (2.1.306). Les constats sont repris ici avec leur correction.

### A1. Boutons des lignes de tâches

- **Constat** : la ligne d'une tâche est rendue cliquable après ses boutons (`Frame::show(..).response`
  puis `interact`) ; egui donne le clic à l'élément le plus haut, la ligne : ▶, ■, ✏ et la case
  « active » ne reçoivent rien. Le README (« ▶ sur la tâche ») ne marche pas.
- **À faire** : ligne cliquable par `UiBuilder::sense` (enregistrée sous ses enfants) ; tests
  d'interface egui_kittest (`tests/ui_*.rs`) sur l'application entière, dans un dossier de données
  temporaire, par de vrais clics.
- **Critère de fin** : test `run_button_launches_the_task` (clic sur ▶ → résultat dans la boîte) ;
  `checkbox_pauses_the_task`.

### A2. Tâche qui se redéclenche elle-même

- **Constat** : une tâche qui écrit dans le dossier qu'elle surveille repart sans fin (14 fois en 8 s
  à l'essai) : les événements arrivent après la fin de l'exécution, quand elle ne « tourne » plus.
  Pour une session Claude, chaque tour est une session payée.
- **À faire** : un fichier dont la date de modification tombe pendant une exécution de la tâche (ou
  dans la seconde qui suit sa fin) est attribué à la tâche et ignoré, noté dans son journal ; un
  fichier modifié après relance la tâche. Limite assumée : une modification de l'utilisateur pendant
  l'exécution est elle aussi ignorée (notée dans le journal).
- **Critère de fin** : tests : une écriture à la toute fin de l'exécution ne relance pas ; une
  modification faite ensuite relance une fois.

### A3. Moteur dans son propre fil

- **Constat** : le moteur n'avance qu'à chaque image de l'interface ; fenêtre réduite, egui ne dessine
  plus et les échéances ne partent pas. L'arrêt (3 s d'attente) peut lancer de nouvelles exécutions.
- **À faire** : le moteur tourne dans un fil (tour toutes les 250 ms, réveil de l'interface quand
  quelque chose change) ; l'interface lit un instantané ; drapeau d'arrêt : plus aucun départ une fois
  la fermeture demandée.
- **Critère de fin** : test du fil (échéance tenue sans interface) ; test `shutdown_starts_nothing`.

### A4. Lancement de `claude`

- **Constat** : la consigne passe en argument : longueur limitée (800 fichiers modifiés → « nom de
  fichier trop long »), refusée par Rust vers un `claude.cmd` dès qu'elle a plusieurs lignes, lue comme
  une option si elle commence par `-`. `claude` seul ne trouve que `claude.exe`.
- **À faire** : consigne écrite dans `consigne.txt` de l'exécution et donnée sur l'entrée standard ;
  `claude.exe` puis `claude.cmd` cherchés dans le `PATH` ; liste des fichiers modifiés plafonnée à 50
  dans la consigne et à 100 dans `PATRONUS_FICHIERS` (avec le total), liste complète dans
  `fichiers.txt` (`PATRONUS_FICHIERS_LISTE`).
- **Critère de fin** : tests avec 800 fichiers ; consigne commençant par `-` ; faux exécutable en
  `.cmd`.

### A5. Tours et budget du vrai Claude Code

- **Constat** : le contrôle du budget en cours de route lit un `total_cost_usd` que le vrai Claude Code
  ne donne qu'à la fin ; le faux exécutable invente un événement. `--max-turns` existe (il n'est pas
  dans l'aide) ; compter les événements `assistant` surestime les tours (un événement par bloc). Une
  session sans événement `result` passe pour réussie ; une session arrêtée n'a pas de coût.
- **À faire** :
  - `--max-turns` ; résultat `error_max_turns` → « arrêtée : nombre de tours atteint » ;
  - estimation du coût pendant la session d'après l'usage de chaque message (dédoublonné par
    `message.id`, tarifs dans `pricing.rs`) ; au-delà du budget, arrêt « budget atteint » ;
    `--max-budget-usd` reste l'arrêt principal ;
  - pas de `result` → échec ; coût estimé gardé (et marqué estimé) quand Patronus arrête la session ;
  - appels d'outils refusés (`permission_denials`) listés dans le résumé ;
  - lecture incrémentale du flux, au plus une fois par seconde ;
  - `faux-claude` conforme : un événement par bloc, même `id`, sortie provisoire, coût seulement à la
    fin, `--max-turns` et `--max-budget-usd` appliqués.
- **Critère de fin** : tests de bout en bout avec le faux exécutable (tours, budget estimé, budget de
  Claude Code, pas de résultat, refus listés).

### A6. Isolation des sessions

- **Constat** : rien n'impose le worktree : une tâche Claude avec `Edit` sans worktree modifie la copie
  de travail ; sans dépôt, elle tourne dans le dossier personnel. Les réglages de l'utilisateur et du
  dépôt (`.claude/settings.json`) élargissent la liste des outils ; `mode_permission` n'est pas
  contrôlé ; `Bash(git push:*)` laisse passer `git -C . push` ; `gh pr comment`, `gh api`… ne sont pas
  interdits.
- **À faire** :
  - outils au-delà de la lecture (Read, Grep, Glob, LS, WebSearch, WebFetch) ou mode `acceptEdits` :
    dépôt et worktree obligatoires (tâche invalide sinon) ; case *worktree* cochée d'office ;
  - `--restricted` (réglages utilisateur, projet et local ignorés, outils de fichiers confinés au
    dossier de travail, `bypassPermissions` refusé), `--strict-mcp-config`, `--tools` (outils
    intégrés de la tâche seulement) ;
  - modes permis : défaut, `acceptEdits`, `plan`, `dontAsk` ;
  - push impossible au niveau de git : `url.patronus-push-interdit:.pushInsteadOf` (vide) injecté par
    `GIT_CONFIG_COUNT` ; `GIT_TERMINAL_PROMPT=0` ;
  - interdits élargis (`git -C … push`, `gh pr create|comment|merge|review`, `gh issue
    create|comment|edit|close`, `gh api`, `gh workflow run`, `gh release`, publications) ;
  - `Bash` sans restriction : avertissement dans l'éditeur.
- **Critère de fin** : tests de validation ; arguments et environnement passés au faux exécutable.

### A7. Processus enfants dans un objet de tâche

- **Constat** : si Patronus s'arrête brutalement, `claude -p` ou le script continue sans surveillance ;
  `taskkill /T` ne suit que les liens de parenté vivants.
- **À faire** : chaque processus lancé dans un objet de tâche Windows
  (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`) ; arrêt par `TerminateJobObject`.
- **Critère de fin** : test : un petit-enfant lancé par le script meurt avec l'arrêt.

### A8. Plusieurs processus sur le même dossier

- **Constat** : `patronus lancer` ouvre un moteur complet : il marque « interrompue » l'exécution en
  cours dans la fenêtre, signale des « manquées », lance des rattrapages qu'il abandonne en quittant ;
  la fenêtre ne voit pas ses résultats avant redémarrage.
- **À faire** :
  - le processus propriétaire (PID et heure de création) est noté dans `run.json` ; une exécution
    « en cours » n'est déclarée interrompue que si son processus a disparu ;
  - mode sans fenêtre (`lancer`, `planifie`) : ni rétention, ni manquées, ni rattrapage, ni
    surveillance ;
  - une exécution à la fois par tâche entre processus : la ligne de commande refuse (« déjà en
    cours ») ; la fenêtre met le départ en attente ;
  - la fenêtre relit `executions\` toutes les 5 s ;
  - README : `patronus lancer` depuis un terminal (`Start-Process -Wait` pour attendre la fin).
- **Critère de fin** : tests à deux moteurs sur un même dossier.

### A9. Rétention et worktrees

- **Constat** : la rétention réglée est ignorée (30 jours en dur, avant que le réglage ne soit lu) ;
  les worktrees gardés ne sont jamais supprimés ; ceux d'une exécution coupée restent orphelins.
- **À faire** : rétention passée à l'ouverture et réappliquée quand le réglage change ; worktree
  supprimé avec son entrée ; au démarrage, chaque worktree sans entrée vivante est rapproché : diff
  enregistré s'il y a des changements (entrée mise à jour), supprimé sinon.
- **Critère de fin** : tests sur dépôt temporaire.

### A10. Interface sobre, brouillon protégé

- **Constat** : le titre de la fenêtre est envoyé à chaque image, ce qui la redessine sans arrêt ; le
  diff est relu et toutes les tâches revalidées à chaque image. Une modification en cours dans
  l'éditeur se perd sans avertissement (Ctrl+N, autre tâche) ; F5 lance la tâche pendant la saisie ;
  ouvrir une exécution en cours la marque lue avant sa fin.
- **À faire** : titre envoyé s'il change ; diff et validation mis en cache ; brouillon gardé quand on
  passe à la boîte, confirmation avant de l'abandonner ; F5 sans effet pendant une saisie ; une
  exécution est marquée lue quand on la regarde une fois terminée.
- **Critère de fin** : test du délai de rafraîchissement ; tests d'interface du brouillon.

### A11. Chemins, sortie de cmd, problèmes

- **Constat** : comparer un chemin accentué au dossier surveillé fait paniquer l'application (coupe au
  milieu d'un caractère) ; la sortie de cmd est en page de code OEM (« caf� ») ; les problèmes sont
  ajoutés à chaque rechargement ; les erreurs de surveillance sont perdues ; un dossier surveillé créé
  après coup n'est pas surveillé.
- **À faire** : comparaison par composants de chemin ; `chcp 65001` devant les commandes cmd ;
  problèmes sans doublons ; erreurs de surveillance signalées ; dossiers manquants revérifiés toutes
  les 30 s.
- **Critère de fin** : tests (chemin accentué, `echo café`, doublons).

### A12. Formats

- **Constat** : les champs inconnus ne sont gardés qu'au premier niveau d'une tâche ; ni `run.json` ni
  `planning.json` n'ont d'état de référence ; supprimer deux fois la même tâche écrase la première mise
  de côté ; un déclencheur connu mais mal écrit est dit « inconnu » ; `$token = …` en PowerShell passe
  pour un secret ; la branche est passée à git sans `--end-of-options`.
- **À faire** : champs inconnus gardés dans l'action, le script, la session, le worktree, les
  garde-fous, la sortie et la surveillance ; états de référence figés (tâche, exécution, planning) ;
  mise de côté horodatée ; message précis par déclencheur ; secret seulement pour une valeur écrite en
  clair ; `--end-of-options`.
- **Critère de fin** : tests.

### A13. Échéances

- **Constat** : après une veille, une échéance part en retard même sans rattrapage ; au-delà de
  100 000 départs manqués la date est fausse ; une tâche ajoutée pendant la fermeture a des
  « manquées » d'avant sa création ; au passage à l'heure d'hiver, la surveillance des échéances se
  tait une heure.
- **À faire** : échéance en retard de plus de 5 min → « manquée » (sauf rattrapage) ; dernière
  échéance calculée directement ; « manquées » comptées à partir de la création du fichier de la
  tâche ; écart de temps pris en valeur absolue.
- **Critère de fin** : tests.

## B. Points remontés après le premier jet

### B1. Symboles de l'interface

- **Constat** : au premier jet, ▶ ■ ✏ et la pastille ● s'affichaient en carrés vides (glyphes absents
  de la police). Corrigé (police d'emoji monochrome d'egui, pastilles dessinées), mais vérifié
  seulement sur des captures réduites.
- **À faire** :
  - tous les symboles de l'interface dans un seul module (`icons.rs`), plus aucun caractère spécial
    écrit en dur ailleurs ;
  - un test vérifie que chaque symbole a un glyphe dans les polices chargées par l'application ;
  - capture agrandie (zoom 200 %, voir C6) de la liste des tâches et de la boîte, regardée à l'œil.
- **Critère de fin** : test `every_symbol_has_a_glyph` ; capture notée dans `journal.md`.

### B2. Déclencheur « branche distante »

- Format : `{"branche_distante": {"branche": "main", "distant": "origin", "toutes_les_minutes": 5}}`
  (`distant` vaut `origin` par défaut, l'intervalle 5 minutes, au moins 1) ; le dépôt est celui de la
  tâche (obligatoire).
- **Détection sans écriture** : `git ls-remote <distant> refs/heads/<branche>`, dans un fil, au plus
  60 s, sans fenêtre ni demande d'identifiants (`GIT_TERMINAL_PROMPT=0`, `GCM_INTERACTIVE=never`).
- Premier relevé : le commit est retenu, sans départ.
- Quand le commit change :
  - `git fetch <distant> <branche>` met à jour les objets et la branche de suivi (`origin/main`),
    jamais la copie de travail ni les branches locales, pour que la session puisse travailler sur
    `origin/main` ;
  - départ avec le déclencheur « origin/main a avancé : abc1234 → def5678 », les variables
    `PATRONUS_BRANCHE`, `PATRONUS_COMMIT_AVANT`, `PATRONUS_COMMIT_APRES`, et la liste des nouveaux
    commits (50 au plus) dans le journal et à la fin de la consigne d'une session Claude.
- Le dernier commit vu est gardé dans `planning.json` : une branche qui a bougé pendant que Patronus
  était fermé déclenche une fois à l'ouverture.
- Une erreur de lecture (réseau, droits, distant inconnu) est signalée une fois dans les problèmes,
  puis réessayée au tour suivant ; elle disparaît quand la lecture réussit.
- **Critère de fin** : tests sur un dépôt « distant » nu et un clone temporaires : premier relevé sans
  départ ; commit poussé → un départ avec la liste des commits ; copie de travail du clone intacte
  (`git status` vide, `HEAD` inchangé) ; distant inexistant → un seul problème signalé.

### B3. Application fermée : tâches du Planificateur de Windows

- **Choix** : une tâche du Planificateur par tâche Patronus qui le demande, plutôt qu'un service
  permanent : rien ne tourne en plus, les heures sont exactes, Windows sait rattraper une échéance
  manquée, et l'utilisateur voit ces tâches dans le Planificateur.
- Case par tâche *Tourner aussi quand Patronus est fermé* (`hors_application`), décochée par défaut ;
  seules les échéances horaires (heure, intervalle) en profitent.
- Tâches Windows dans le dossier `\Patronus\` du Planificateur (`\Patronus-<empreinte>\` pour un
  autre dossier de données), dans la session de l'utilisateur, sans droit administrateur ; une seule
  instance à la fois ; rattrapage « dès que possible » si la tâche le demande ; arrêt par Windows en
  dernier recours (durée maximale + 10 min).
- Commande inscrite : `patronus.exe --donnees <dossier> planifie <tâche>`. Elle ne fait rien si la
  fenêtre de ce dossier est ouverte (elle tient ses échéances), ni si l'échéance a déjà été tenue ;
  sinon elle lance la tâche, attend sa fin, et le résultat arrive dans la boîte.
- Synchronisation à l'ouverture et après chaque modification de tâche, dans un fil : création,
  remplacement (exécutable déplacé compris), suppression des tâches Windows qui n'ont plus lieu
  d'être.
- À l'ouverture de la fenêtre, une échéance tenue par le Planificateur n'est plus signalée
  « manquée ».
- **Critère de fin** : tests du XML, de la lecture de `schtasks /Query`, de la décision de
  `planifie` ; test ignoré d'inscription réelle, lancé une fois ; essai réel noté dans `journal.md`
  (tâche à deux minutes, fenêtre fermée, résultat dans la boîte à la réouverture).

### B4. Premier essai réel de l'action Claude

- Par l'utilisateur, ou par Claude avec son accord explicite (coût sur son compte).
- Préparé : modèle *Essai Claude (lecture seule)* (C2) : à la main, outils en lecture seule, sans
  worktree, 0,10 $ au plus, 5 tours ; liste de vérification dans le README (résumé, coût affiché,
  journal, arrêt par le budget et par les tours).

## C. Fonctionnalités

### C1. Actions sur un résultat

- **Appliquer le diff** : `git apply --check` d'abord ; puis confirmation (dépôt, branche courante,
  fichiers touchés) ; `git apply` dans la copie de travail seulement (ni index, ni commit). Refus
  expliqué si la copie de travail a divergé. L'entrée note « diff appliqué le … ».
- **Supprimer le worktree** d'un résultat (confirmation ; le diff reste dans l'entrée) ; les worktrees
  sont aussi supprimés avec les entrées que la rétention efface.
- **Reprendre la session** Claude dans un terminal, dans son dossier (worktree ou dépôt) :
  `claude --resume <session>`, dans Windows Terminal s'il est installé, sinon PowerShell ; dans Fawkes
  quand le contrat local existera.
- *Copier le résumé* ; liens cliquables (C5).
- **Critère de fin** : tests sur un dépôt temporaire (diff appliqué : fichier présent, index vide ;
  copie de travail divergente : refus ; suppression du worktree ; rétention) ; test d'interface
  (*Appliquer le diff* → confirmation → appliqué).

### C2. Modèles de tâches

- *Nouvelle tâche* propose : vide (script), *Point du matin*, *Revue des dépendances*, *Doc à jour
  après une fusion* (branche distante), *Surveiller un dossier*, *Tests réguliers*, *Essai Claude
  (lecture seule)*. Le dépôt choisi pré-remplit le modèle.
- **Critère de fin** : chaque modèle, dépôt rempli, est valide ; test d'interface (tâche créée depuis
  un modèle).

### C3. Budget mensuel des sessions Claude

- Réglage *Budget mensuel des sessions Claude* (aucun par défaut).
- Coût du mois tenu dans `planning.json`, indépendamment de la rétention des résultats.
- Une session dont le budget ferait dépasser le budget mensuel ne part pas : entrée « impossible :
  budget mensuel atteint (x $ sur y $) ».
- Barre d'état : « Ce mois : 1,23 $ (équivalent API) ».
- **Critère de fin** : tests du moteur (cumul, mois suivant, refus).

### C4. Démarrage avec Windows, fermeture pendant une exécution

- Réglage *Lancer Patronus à l'ouverture de session* (clé `Run` du compte, sans droit administrateur,
  décoché par défaut), fenêtre réduite (`--reduit`).
- Fermer pendant une exécution : « 2 exécutions sont en cours. » *Arrêter et quitter* / *Annuler* ;
  leurs entrées disent « interrompue : Patronus a été fermé ».
- **Critère de fin** : test du registre sur une clé d'essai ; test d'interface de la fermeture.

### C5. Résumé et liens écrits par le script

- `PATRONUS_RESUME` : chemin d'un fichier ; s'il est rempli à la fin, son contenu devient le résumé
  (au lieu des dernières lignes de la sortie).
- Les liens `http(s)://` du résumé et de la fin du journal deviennent des liens cliquables (10 au plus).
- **Critère de fin** : tests de l'exécution des scripts.

### C6. Petits plus

- **Pause générale** : *Tout mettre en pause* ; aucun départ automatique, le lancement à la main
  reste possible ; affiché dans la barre d'état et retenu au redémarrage (les échéances passées
  pendant la pause ne sont pas « manquées »).
- **Boîte** : recherche (tâche, résumé, déclencheur), *Tout marquer comme lu*.
- *Dupliquer* une tâche.
- **Zoom** de l'interface (réglage, Ctrl+molette, Ctrl+0).
- **Icône** (tête de cerf) et informations de version de l'exécutable.
- **Palette** : flèches haut et bas, Entrée.
- **Critère de fin** : tests (pause, recherche) ; capture.

## Pistes (plus tard)

- Tâches propres à un dépôt (`.patronus/<tâche>.json`), avec accord avant activation.
- *Ouvrir une PR* depuis un résultat (après validation), commentaire de PR (Owlery).
- Contrat local par canal nommé (`contrat-local.md`) : demandes de Fawkes, DragonTree, Owlery,
  Menagerie ; reprise d'une session dans Fawkes.
- Masquage des secrets dans les journaux, avec les règles de Fawkes.
- Enchaînement de tâches ; routines et sessions de fond de Claude Code affichées.
- Notification cliquable qui ouvre l'entrée ; icône de la zone de notification rétablie après un
  redémarrage de l'Explorateur.
