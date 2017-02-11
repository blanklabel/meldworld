package model

import (
	"github.com/blanklabel/meldworld/model"
)

type WorldMap struct {
	Type string `json:"type"`
	model.MapObj
	model.EntityObj
}

// Size of a map
type Dimension struct {
	Height int
	Width  int
}

// Container for the jason
type MapObj struct {
	Map Dimension
}
