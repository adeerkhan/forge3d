"""HDRI sky model acceptance.

Studio's Rendered view draws the project HDRI as its background by asking the
terrain renderer for ``SkySettings(model="hdri")``. The map itself travels with
the IBL, so the model carries no path; only the exposure (background intensity)
is read from these settings.
"""

import pytest

from forge3d.terrain_params import SkySettings


def test_hdri_sky_model_keeps_the_physical_defaults():
    sky = SkySettings(enabled=True, model="hdri", sky_exposure=0.4, aerial_perspective=False)
    assert sky.model == "hdri"
    assert sky.sky_exposure == 0.4
    # The analytic parameters are still normalised so a caller can switch
    # models without filling every field.
    assert sky.turbidity == 2.0
    assert sky.ground_albedo == 0.3


def test_hdri_sky_requires_a_known_model_name():
    with pytest.raises(ValueError, match="model must be one of"):
        SkySettings(enabled=True, model="hdri-ish")


def test_hdri_sky_exposure_must_be_finite_and_non_negative():
    with pytest.raises(ValueError, match="sky_exposure"):
        SkySettings(enabled=True, model="hdri", sky_exposure=-0.1)
    with pytest.raises(ValueError, match="sky_exposure"):
        SkySettings(enabled=True, model="hdri", sky_exposure=float("nan"))
