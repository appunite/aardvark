import numpy as np
import sklearn
from sklearn.datasets import load_iris
from sklearn.linear_model import LogisticRegression
from sklearn.metrics import accuracy_score


def train_model():
    data = load_iris()
    x, y = data.data, data.target
    model = LogisticRegression(max_iter=200, solver="liblinear")
    model.fit(x, y)
    preds = model.predict(x)
    return float(accuracy_score(y, preds))


def handler(event=None, context=None):
    accuracy = train_model()
    return {
        "sklearn_version": sklearn.__version__,
        "numpy_version": np.__version__,
        "accuracy": accuracy,
    }


def main():
    return handler()


if __name__ == "__main__":
    import json

    print(json.dumps(main(), sort_keys=True))
