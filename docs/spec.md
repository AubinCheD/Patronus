# Patronus — spécification

Reprise et approfondissement de la réflexion du 21/10/2026 sur les agents autonomes et les tâches
récurrentes (Fawkes, `docs/reflexions.md` §4).

## 1. Principe

Une **tâche** réunit quatre choses :

1. un **déclencheur** : quand elle part ;
2. une **action** : ce qu'elle fait ;
3. des **garde-fous** : ce qu'elle n'a pas le droit de faire, et ses limites ;
4. une **sortie** : où arrive le résultat, et ce qu'on peut en faire.

Une **exécution** est un passage d'une tâche : début, fin, journal, coût, résultat.

## 2. Déclencheurs

| Déclencheur | Exemple | Détail |
|---|---|---|
| Heure | tous les jours à 8 h ; toutes les heures ; le lundi à 9 h | Expression simple (jours, heures) plutôt qu'une syntaxe cron brute ; rattrapage facultatif d'une exécution manquée (PC éteint). |
| Fichier modifié | `docs/**` ; `package.json` | Surveillance du dossier, regroupement des changements rapprochés (quelques secondes) ; motifs d'inclusion et d'exclusion. |
| Nouveau commit, branche distante qui bouge | `origin/main` avance | Lecture périodique (`git fetch` léger) ou crochet local ; jamais d'écriture dans le dépôt de travail. |
| PR ouverte ou commentée | nouvelle PR sur `contoso/api` | Via les forges (GitHub, GitLab, Azure DevOps), comptes partagés avec DragonTree. |
| CI en échec | pipeline rouge sur `main` | Même source que la boîte de réception (Owlery). |
| Motif dans la sortie d'un terminal | « Unhandled exception » dans les journaux d'un service | Watcher : Fawkes signale le motif (expression simple), Patronus déclenche. |
| Service arrêté | le serveur `npm run dev` est tombé | Signalé par Fawkes ou Menagerie. |
| À la main | bouton *Lancer maintenant*, palette, ligne de commande | Toujours possible, même pour une tâche planifiée. |

Une tâche peut avoir plusieurs déclencheurs. Une seule exécution à la fois par tâche : un
déclenchement pendant une exécution est mis en attente (une seule fois), pas empilé.

## 3. Actions

1. **Session Claude non interactive** : `claude -p` avec :
   - la consigne (texte de la tâche, éventuellement complété par le déclencheur : le diff, la PR, la
     ligne de journal) ;
   - les outils permis (`--allowedTools`), le mode de permission ;
   - un nombre maximal de tours et un budget ;
   - un dossier de travail : un **worktree dédié** créé pour l'exécution, sur la branche choisie,
     supprimé à la fin si rien n'est à garder.
   La sortie (`--output-format stream-json`) est enregistrée et résumée.
2. **Script** : une commande (PowerShell, bash, cmd) avec son dossier et ses variables ; sortie et
   code de retour enregistrés.
3. **Session interactive dans Fawkes** : la tâche ouvre une session Claude dans le groupe « Agents »
   de Fawkes, avec la consigne collée sans être envoyée ; utile quand on veut garder la main.

## 4. Sorties

- **Boîte de résultats** : une entrée par exécution, avec :
  - un résumé court (ce qui a été fait, ce qui reste) ;
  - le diff produit (fichiers, lignes), consultable ;
  - les liens (PR, issue, commit, fichier) ;
  - la durée, le coût (tokens, budget consommé), le journal complet.
- **Actions proposées**, jamais automatiques : appliquer le diff dans la copie de travail, ouvrir une
  PR, commenter une PR, reprendre la session dans Fawkes, archiver.
- **Notification** Windows à la fin (réglable par tâche), avec un lien vers l'entrée.
- **Rien en silence** : une exécution qui échoue ou dépasse ses limites laisse une entrée qui dit
  pourquoi.

## 5. Garde-fous

| Règle | Application |
|---|---|
| Pas de push, fusion, déploiement ou publication sans validation | Les outils correspondants ne sont pas permis à l'agent ; l'action proposée passe par un clic. |
| Worktree isolé | L'agent ne touche jamais à la copie de travail de l'utilisateur. |
| Outils limités par tâche | Liste explicite (lecture, édition, tests…) ; par défaut, lecture seule. |
| Budget et durée maximum | Arrêt propre au dépassement, entrée « arrêtée : budget atteint ». |
| Une exécution à la fois par tâche | Voir §2. |
| Journal | Tout est enregistré : consigne, outils appelés, sorties, coût. |
| Bouton d'arrêt | Par exécution, et global (« tout arrêter »). |
| Aucun secret dans les consignes | Les secrets passent par les variables d'environnement ou les gestionnaires de secrets, jamais dans le texte de la tâche. |
| Confirmation d'une tâche importée | Une tâche venue d'un dépôt cloné ne s'active pas sans accord (même principe que l'espace de travail de Fawkes). |

## 6. Exemples

- **Revue quotidienne des dépendances** : chaque matin, liste des mises à jour disponibles et des
  avis de sécurité, avec un diff proposé pour les mises à jour mineures.
- **Tri des nouvelles issues** : étiquettes et résumé proposés, sans les appliquer.
- **Surveillance des journaux d'un service** : quand une exception apparaît dans un terminal de Fawkes,
  Patronus lance une analyse et propose un correctif dans un worktree.
- **Doc à jour après une fusion** : quand `main` avance, vérifier que la doc décrit encore le code ;
  proposer une PR de mise à jour.
- **Point du matin** : commits de la veille, PR en attente, CI en échec, sessions Claude laissées en
  cours.

## 7. Interface

- **Liste des tâches** : nom, déclencheurs, dernière exécution (état, date), prochaine exécution,
  interrupteur actif / en pause.
- **Éditeur de tâche** : formulaire (déclencheurs, action, garde-fous, sortie) ; aperçu du fichier.
- **Boîte de résultats** : entrées non lues en tête, filtre par tâche et par dépôt ; vue détaillée
  avec résumé, diff, journal et actions.
- **Exécutions en cours** : progression, coût consommé, *Arrêter*, *Ouvrir dans Fawkes*.
- **Palette** : *Lancer la tâche…*, *Mettre en pause…*, *Ouvrir la boîte de résultats*.

## 8. Données

- **Tâches** : un fichier par tâche, lisible et versionnable ; globales (dossier de l'application)
  ou propres à un dépôt (`.patronus/<tâche>.json` à la racine, confirmation avant activation).
- **Exécutions** : dossier de l'application, une entrée par exécution (métadonnées, journal, diff),
  avec rétention réglable.
- **État** : versionné comme celui de Fawkes (version de l'application qui l'a écrit, copie avant
  mise à jour, champs inconnus gardés, état de référence figé dans les tests).

Exemple de tâche (fictif) :

```json
{
  "nom": "Revue des dépendances",
  "depot": "C:\\dev\\contoso-api",
  "declencheurs": [{ "heure": "08:00", "jours": ["lun", "mar", "mer", "jeu", "ven"] }],
  "action": {
    "claude": {
      "consigne": "Liste les dépendances à mettre à jour et les avis de sécurité ; propose un diff pour les mises à jour mineures.",
      "outils": ["Read", "Grep", "Bash(npm outdated)", "Edit"],
      "tours_max": 20,
      "budget_usd": 1.0
    },
    "worktree": { "branche": "main" }
  },
  "sortie": { "notification": true }
}
```

## 9. Où ça tourne

- L'interface dans l'application (ou le module de la suite) ; l'exécution des sessions visibles dans
  Fawkes quand on le souhaite.
- **Quand l'application est fermée** : un petit service ou une tâche planifiée Windows lance les
  exécutions dues ; l'interface les retrouve dans la boîte au démarrage.
- **Claude Code** a ses propres sessions en arrière-plan et des routines programmées : à intégrer
  (les afficher, les lancer) plutôt qu'à refaire.

## 10. Intégrations

- **Fawkes** : ouverture d'une session dans le groupe « Agents » ; watchers sur la sortie des
  terminaux ; « Patronus » dans la palette de Fawkes.
- **DragonTree** : *Lancer une tâche sur ce dépôt* ; diff d'un résultat ouvert dans DragonTree.
- **Owlery** : déclencheurs PR et CI partagés ; un résultat peut devenir un commentaire de PR (après
  validation).
- **Menagerie** : déclencheur « service arrêté ».
- **Contrat local** : demandes reçues et émises par le canal nommé commun.

## 11. Questions ouvertes

- Faut-il un service Windows permanent, ou une tâche planifiée par tâche suffit-elle ?
- Où ranger les tâches propres à un dépôt sans polluer le dépôt (dossier dédié, fichier ignoré) ?
- Comment présenter le coût (tokens, euros) quand le compte est un abonnement et non une clé d'API ?
- Faut-il permettre l'enchaînement de tâches (le résultat de l'une déclenche l'autre) ?
- Quelle rétention par défaut pour les journaux d'exécution ?
