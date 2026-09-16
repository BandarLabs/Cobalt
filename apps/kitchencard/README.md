# Kitchen Card

![Tonight's recipe card on a Kobo Clara BW panel](screenshots/tonight.png)

Kitchen Card is an unofficial, read-only Mealie companion. Pick tonight's
recipe, then cook one large instruction at a time; the left and right page
zones move through steps and the Ingredients tab stays one tap away. The
selected card and servings survive offline use.

Set the Mealie LAN endpoint and its long-lived API token outside the app:

```sh
kobo secret set mealie
```

The app asks the runtime to attach that token as an `Authorization` header, so
it is never in app-visible state. Mealie is AGPL-3.0. This app is unofficial
and does not edit recipes, meal plans, or shopping lists.

`drive.kobo` opens a recipe, starts cooking, and captures the product screen.

## On the device

Synced against a live Mealie server with real recipes.

<table><tr>
<td><img width="300" src="screenshots/tonight.png" alt="Tonight's card for a synced recipe, with servings, step and ingredient counts"><br>Tonight's card for a synced recipe</td>
<td><img width="300" src="screenshots/cooking.png" alt="Cooking view showing one large instruction and the Next step button"><br>Cooking, one large step at a time</td>
</tr><tr>
<td><img width="300" src="screenshots/ingredients.png" alt="Ingredients tab listing real ingredient lines with check-off circles"><br>Ingredients, one tap away</td>
<td><img width="300" src="screenshots/browse.png" alt="Browse view listing every synced recipe with servings and step counts"><br>Every synced recipe, browsable offline</td>
</tr></table>
